//! The main DHCP socket module.

use std::net::SocketAddr;
use core::task::Context;
use core::pin::Pin;

use futures::task::Poll;
use futures::ready;
use tokio::{io, net::UdpSocket};

/// UDP I/O trait
pub trait UdpAsyncReadWrite {
    fn poll_recv_from(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<Result<(usize, SocketAddr), io::Error>>;
    fn poll_send_to(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8], target: &SocketAddr) -> Poll<Result<usize, io::Error>>;
}

impl UdpAsyncReadWrite for UdpSocket {
    fn poll_recv_from(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<Result<(usize, SocketAddr), io::Error>> {
        let this = Pin::into_inner(self);
        let mut buf = tokio::io::ReadBuf::new(buf);
        match ready!(this.poll_recv_from(cx, &mut buf)) {
            Ok(sockaddr) => {
                let len = buf.filled().len();
                Poll::Ready(Ok((len, sockaddr)))
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_send_to(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8], target: &SocketAddr) -> Poll<Result<usize, io::Error>> {
        let this = Pin::into_inner(self);
        this.poll_send_to(cx, buf, target.clone())
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
    R: Unpin + UdpAsyncReadWrite,
    U: Unpin + UdpAsyncReadWrite,
    MR: Unpin,
    MU: Unpin,
{
    /// Receives data from the socket. On success, returns the number of bytes read and the address from whence the data came.
    /// #Panics
    /// This function will panic if called outside the context of a future's task.
    fn poll_recv_from(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<Result<(usize, SocketAddr), io::Error>> {
        let this = Pin::into_inner(self);
        match &mut this.socket {
            Socket::Udp(socket) => Pin::new(socket).poll_recv_from(cx, buf),
            Socket::Raw(socket) => {
                // TODO: make raw socket transient.
                // that is create it only for the duration of sending or receiving
                // to reduce resource consumption (it captures all IP packets)
                Pin::new(socket).poll_recv_from(cx, buf)
            }
        }
    }

    fn poll_send_to(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8], target: &SocketAddr) -> Poll<Result<usize, io::Error>> {
        let this = Pin::into_inner(self);
        match &mut this.socket {
            Socket::Udp(socket) => Pin::new(socket).poll_send_to(cx, buf, target),
            Socket::Raw(socket) => {
                // TODO: make raw socket transient.
                // that is create it only for the duration of sending or receiving
                // to reduce resource consumption (it captures all IP packets)
                Pin::new(socket).poll_send_to(cx, buf, target)
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
