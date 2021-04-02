//! The main DHCP socket module.

use std::task::Context;
use std::{net::SocketAddr, pin::Pin};

use pin_project::pin_project;

use futures::{Sink, Stream, ready};
use futures::task::Poll;
use tokio::io;

use dhcp_protocol::Message;
use switchable_socket::{
    MakeSocket, ModeSwitch, SocketMode, SwitchableUdpSocket, UdpAsyncReadWrite,
};

/// Must be enough to decode all the options.
pub const BUFFER_READ_CAPACITY: usize = 8192;
/// Must be enough to encode all the options.
pub const BUFFER_WRITE_CAPACITY: usize = 8192;

/// The modified version of the `tokio::UdpFramed`.
///
/// Works with high level DHCP messages.
#[pin_project]
pub struct DhcpFramed<S> {
    /// `tokio::UdpSocket`.
    #[pin]
    socket: S,
    /// Stores received data and is used for deserialization.
    buf_read: Vec<u8>,
    /// Stores pending data and is used for serialization.
    buf_write: Vec<u8>,
    /// Stores the destination address and the number of bytes to send.
    pending: Option<(SocketAddr, usize)>,
}

pub type DhcpStreamItem = (SocketAddr, Message);
pub type DhcpSinkItem = (SocketAddr, (Message, Option<u16>));

impl<S> DhcpFramed<S> {
    /// Binds to `addr` and returns a `Stream+Sink` UDP socket abstraction.
    ///
    /// # Errors
    /// `io::Error` on unsuccessful socket building or binding.
    #[allow(unused_variables)]
    pub fn new(socket: S) -> io::Result<Self> {
        Ok(DhcpFramed {
            socket,
            buf_read: vec![0u8; BUFFER_READ_CAPACITY],
            buf_write: vec![0u8; BUFFER_WRITE_CAPACITY],
            pending: None,
        })
    }
}

impl<R, U, MR, MU> ModeSwitch for DhcpFramed<SwitchableUdpSocket<R, U, MR, MU>>
where
    MR: MakeSocket<Socket = R>,
    MU: MakeSocket<Socket = U>,
{
    fn switch_to(&mut self, mode: SocketMode) -> Result<(), io::Error> {
        self.socket.switch_to(mode)
    }
    fn mode(&self) -> SocketMode {
        self.socket.mode()
    }
    fn device_name(&self) -> &str {
        self.socket.device_name()
    }
}

impl<S> Stream for DhcpFramed<S>
where
    S: UdpAsyncReadWrite,
{
    type Item = Result<DhcpStreamItem, io::Error>;

    /// Returns `Ok(Async::Ready(Some(_)))` on successful
    /// both read from socket and decoding the message.
    /// Returns `Ok(Async::Ready(None))` a on parsing error.
    ///
    /// # Errors
    /// `io::Error` on a socket error.
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let mut this = self.as_mut().project();
            match ready!(this.socket.poll_recv_from(cx, &mut this.buf_read)) {
                Ok((amount, addr)) => {
                    match Message::from_bytes(&this.buf_read[..amount]) {
                        Ok(frame) => return Poll::Ready(Some(Ok((addr, frame)))),
                        Err(_) => {
                            log::error!("Malformed DHCP frame");
                            // continue and try to read next packet
                        }
                    }
                }
                Err(e) => {
                    return Poll::Ready(Some(Err(e)));
                }
            }
        } // end loop
    }
}

impl<S> DhcpFramed<S>
where
    S: UdpAsyncReadWrite,
{
    // tries to send data immediately
    fn try_send_to(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>, addr: SocketAddr, amount: usize) -> Poll<Result<(), io::Error>> {
        let this = self.project();
        match ready!(this.socket.poll_send_to(cx, &this.buf_write[..amount], &addr)) {
            Ok(sent) => {
                *this.pending = None;
                if sent != amount {
                    log::error!("Datagram was truncated");
                    Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::Other,
                        format!("Failed to write entire datagram to socket. Amount: {}, sent: {}", amount, sent),
                    )))
                } else {
                    Poll::Ready(Ok(()))
                }
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl<S> Sink<DhcpSinkItem> for DhcpFramed<S>
where
    S: UdpAsyncReadWrite,
{
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        if let Some((addr, amount)) = self.pending {
            self.try_send_to(cx, addr, amount)
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn start_send(self: Pin<&mut Self>, item: DhcpSinkItem) -> Result<(), Self::Error> {
        // TODO: some DHCP servers drop packets above 576 ethernet octets (562 IP octects)
        // and DFC 1542 requires the minimum packet size of 300 BOOTP bytes.
        let (addr, (message, max_size)) = item;
        let this = self.project();
        let amount = message.to_bytes(this.buf_write, max_size)?;

        // We don't check the value of self.pending, poll_ready returning Ok(()) guaranties that it's None
        *this.pending = Some((addr, amount));
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        if let Some((addr, amount)) = self.pending {
            self.try_send_to(cx, addr, amount)
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        if let Some((addr, amount)) = self.pending {
            self.try_send_to(cx, addr, amount)
        } else {
            Poll::Ready(Ok(()))
        }
    }
}
