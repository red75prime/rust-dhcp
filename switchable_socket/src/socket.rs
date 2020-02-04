//! The main DHCP socket module.

use std::net::SocketAddr;

use tokio::{io, net::UdpSocket, prelude::*};

/// UDP I/O trait
pub trait UdpAsyncReadWrite {
    fn poll_recv_from(&mut self, buf: &mut [u8]) -> Poll<(usize, SocketAddr), io::Error>;
    fn poll_send_to(&mut self, buf: &[u8], target: &SocketAddr) -> Poll<usize, io::Error>;
}

impl UdpAsyncReadWrite for UdpSocket {
    fn poll_recv_from(&mut self, buf: &mut [u8]) -> Poll<(usize, SocketAddr), io::Error> {
        UdpSocket::poll_recv_from(self, buf)
    }

    fn poll_send_to(&mut self, buf: &[u8], target: &SocketAddr) -> Poll<usize, io::Error> {
        UdpSocket::poll_send_to(self, buf, target)
    }
}

pub trait MakeSocket {
    type Socket;
    fn make(&mut self, iface: &str, port: u16) -> Result<Self::Socket, io::Error>;
}

impl<T, F> MakeSocket for F
where
    F: FnMut(&str, u16) -> Result<T, io::Error>,
{
    type Socket = T;
    fn make(&mut self, iface: &str, port: u16) -> Result<T, io::Error> {
        (self)(iface, port)
    }
}

/// Socket mode
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SocketMode {
    Raw,
    Udp,
}

/// Allows socket mode switch
pub trait ModeSwitch {
    /// Switches socket mode
    fn switch_to(&mut self, mode: SocketMode) -> Result<(), io::Error>;
    /// Returns current socket mode
    fn mode(&self) -> SocketMode;
    /// Device (interface) name this socket is bound to
    fn device_name(&self) -> &str;
}

/// Either UDP or RAW socket
enum Socket<R, U> {
    Raw(R),
    Udp(U),
}

impl<R, U> Socket<R, U> {
    fn mode(&self) -> SocketMode {
        match self {
            Socket::Udp(_) => SocketMode::Udp,
            Socket::Raw(_) => SocketMode::Raw,
        }
    }
}

/// Socket that can switch between raw and udp modes
/// Raw mode allows packet reception on a network interface
/// with no assigned IP address
pub struct SwitchableUdpSocket<R, U, MR, MU> {
    iface: String,
    port: u16,
    socket: Socket<R, U>,
    make_raw: MR,
    make_udp: MU,
}

impl<R, U, MR, MU> SwitchableUdpSocket<R, U, MR, MU> {
    /// Creates a socket in a specified mode that is bound to `port`, and INADDR_ANY.
    /// `make_raw` and `make_udp` are used to create underlaying sockets.
    pub fn new(
        iface: &str,
        port: u16,
        mode: SocketMode,
        mut make_raw: MR,
        mut make_udp: MU,
    ) -> Result<SwitchableUdpSocket<R, U, MR, MU>, io::Error>
    where
        R: UdpAsyncReadWrite,
        U: UdpAsyncReadWrite,
        MR: MakeSocket<Socket = R>,
        MU: MakeSocket<Socket = U>,
    {
        let socket = new_socket(iface, port, mode, &mut make_raw, &mut make_udp)?;
        Ok(SwitchableUdpSocket {
            iface: iface.to_string(),
            port,
            socket,
            make_raw,
            make_udp,
        })
    }
}

impl<R, U, MR, MU> UdpAsyncReadWrite for SwitchableUdpSocket<R, U, MR, MU>
where
    R: UdpAsyncReadWrite,
    U: UdpAsyncReadWrite,
{
    /// Receives data from the socket. On success, returns the number of bytes read and the address from whence the data came.
    /// #Panics
    /// This function will panic if called outside the context of a future's task.
    fn poll_recv_from(&mut self, buf: &mut [u8]) -> Result<Async<(usize, SocketAddr)>, io::Error> {
        match &mut self.socket {
            Socket::Udp(socket) => socket.poll_recv_from(buf),
            Socket::Raw(socket) => {
                // TODO: make raw socket transient.
                // that is create it only for the duration of sending or receiving
                // to reduce resource consumption (it captures all IP packets)
                socket.poll_recv_from(buf)
            }
        }
    }

    fn poll_send_to(&mut self, buf: &[u8], target: &SocketAddr) -> Result<Async<usize>, io::Error> {
        match &mut self.socket {
            Socket::Udp(socket) => socket.poll_send_to(buf, target),
            Socket::Raw(socket) => {
                // TODO: make raw socket transient.
                // that is create it only for the duration of sending or receiving
                // to reduce resource consumption (it captures all IP packets)
                socket.poll_send_to(buf, target)
            }
        }
    }
}

impl<R, U, MR, MU> ModeSwitch for SwitchableUdpSocket<R, U, MR, MU>
where
    MR: MakeSocket<Socket = R>,
    MU: MakeSocket<Socket = U>,
{
    fn switch_to(&mut self, mode: SocketMode) -> Result<(), io::Error> {
        debug!("Entering listen mode {:?}", mode);
        if mode != self.mode() {
            debug!("Switching listen mode to {:?}", mode);
            self.socket = new_socket(
                &self.iface,
                self.port,
                mode,
                &mut self.make_raw,
                &mut self.make_udp,
            )?;
        }
        Ok(())
    }

    fn mode(&self) -> SocketMode {
        self.socket.mode()
    }

    fn device_name(&self) -> &str {
        &self.iface
    }
}

fn new_socket<R, U>(
    iface: &str,
    port: u16,
    mode: SocketMode,
    make_raw: &mut dyn MakeSocket<Socket = R>,
    make_udp: &mut dyn MakeSocket<Socket = U>,
) -> Result<Socket<R, U>, io::Error> {
    match mode {
        SocketMode::Raw => {
            let socket = make_raw.make(iface, port)?;
            Ok(Socket::Raw(socket))
        }
        SocketMode::Udp => {
            let socket = make_udp.make(iface, port)?;
            Ok(Socket::Udp(socket))
        }
    }
}
