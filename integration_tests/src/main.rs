#[macro_use]
extern crate log;

use std::{
    io,
    net::{Ipv4Addr},
};

use eui48::MacAddress;
use futures::{Future, Sink, Stream, StreamExt};

use dhcp_client::Client;
use dhcp_framed::{
    DhcpSinkItem, DhcpStreamItem
};
use dhcp_protocol::{MessageType};
use switchable_socket::{ModeSwitch, SocketMode};
use futures::channel::mpsc::{self, Sender, Receiver};
use std::task::{Context, Poll};
use std::pin::Pin;
use pin_project::pin_project;

#[pin_project]
struct UnreliableChannel<F> {
    filter: F,
    mode: SocketMode,
    #[pin]
    sender: Sender<DhcpStreamItem>,
    #[pin]
    receiver: Receiver<DhcpStreamItem>,
}

impl<F> Stream for UnreliableChannel<F>
where
    F: 'static + Send + Sync + FnMut(&DhcpStreamItem) -> bool,
{
    type Item = Result<DhcpStreamItem, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let item = match self.as_mut().project().receiver.poll_next(cx) {
                Poll::Pending => {
                    return Poll::Pending;
                }
                Poll::Ready(Some(item)) => {
                    item
                }
                Poll::Ready(None) => {
                    return Poll::Ready(Some(Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Channel closed"))));
                }
            };
            info!("Filtering {:?}", item.1.options.dhcp_message_type);
            if !(self.as_mut().project().filter)(&item) {
                warn!("Dropping {}", item.1);
                // continue
            } else {
                return Poll::Ready(Some(Ok(item)));
            }
        }
    }
}

impl<F> Sink<DhcpSinkItem> for UnreliableChannel<F>
where
    F: 'static + Send + Sync + FnMut(&DhcpStreamItem) -> bool,
{
    type Error = io::Error;

    fn start_send(mut self: Pin<&mut Self>, item: DhcpSinkItem) -> io::Result<()> {
        let (socket, (msg, _size)) = item;
        let si = (socket, msg);
        if !(self.as_mut().project().filter)(&si) {
            warn!("Dropping {}", si.1);
            return Ok(());
        }
        match self.sender.start_send(si) {
            Ok(()) => {
                Ok(())
            }
            Err(_) => {
                Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Channel closed"))
            }
        }
    }

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().sender.poll_ready(cx).map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "Channel closed"))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().sender.poll_flush(cx).map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "Channel closed"))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().sender.poll_close(cx).map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "Channel closed"))
    }
}

impl<F> ModeSwitch for UnreliableChannel<F> {
    fn switch_to(self: Pin<&mut Self>, mode: SocketMode) -> Result<(), io::Error> {
        *self.project().mode = mode;
        Ok(())
    }

    fn mode(self: Pin<&mut Self>) -> SocketMode {
        *self.project().mode
    }

    fn device_name(self: Pin<&mut Self>) -> &str {
        "dummy"
    }
}

fn always_pass(_: &DhcpStreamItem) -> bool {
    true
}

fn unreliable_channel<F>(filter: F) -> (UnreliableChannel<F>, UnreliableChannel<impl FnMut(&DhcpStreamItem) -> bool>)
where
    F: 'static + Send + Sync + FnMut(&DhcpStreamItem) -> bool,
{
    let (client_tx, server_rx) = mpsc::channel(2);
    let (server_tx, client_rx) = mpsc::channel(2);

    (
        UnreliableChannel {
            filter,
            mode: SocketMode::Raw,
            sender: client_tx,
            receiver: client_rx,
        },
        UnreliableChannel {
            filter: always_pass,
            mode: SocketMode::Raw,
            sender: server_tx,
            receiver: server_rx,
        },
    )
}

fn client<F>(channel: UnreliableChannel<F>) -> impl Future<Output = ()>
where
    F: 'static + Send + Sync + FnMut(&DhcpStreamItem) -> bool,
{

    // let iface_str = "ens33";

    let server_address = None;
    let client_address = None;
    let address_request = Some(Ipv4Addr::new(192, 168, 0, 60));
    let address_time = Some(60);
    let request_static_routes = false;

    let client = Client::new(
        channel,
        MacAddress::new([0x00, 0x0c, 0x29, 0x13, 0x0e, 0x37]),
        None,
        None,
        server_address,
        client_address,
        address_request,
        address_time,
        None,
        request_static_routes,
    );

    async move {
        futures::pin_mut!(client);
        loop {
            match client.next().await {
                Some(Err(error)) => {
                    error!("Error: {}", error);
                }
                Some(Ok(msg)) => {
                    info!("Client: {:?}", msg);
                }
                None => {
                    break;
                }
            }
        }
    }
}

fn server<F>(channel: UnreliableChannel<F>) -> impl Future<Output = ()>
where
    F: 'static + Send + Sync + FnMut(&DhcpStreamItem) -> bool,
{
    let server_ip_address = Ipv4Addr::new(192, 168, 0, 2);
    let iface_name = "Ethernet".to_string();

    #[allow(unused_mut)]
    let mut builder = dhcp_server::ServerBuilder::new(
        server_ip_address,
        iface_name,
        (
            Ipv4Addr::new(192, 168, 0, 50),
            Ipv4Addr::new(192, 168, 0, 99),
        ),
        (
            Ipv4Addr::new(192, 168, 0, 100),
            Ipv4Addr::new(192, 168, 0, 199),
        ),
        dhcp_server::RamStorage::new(),
        Ipv4Addr::new(255, 255, 0, 0),
        vec![Ipv4Addr::new(192, 168, 0, 1)],
        vec![Ipv4Addr::new(192, 168, 0, 1)],
        vec![(Ipv4Addr::new(192, 168, 0, 0), Ipv4Addr::new(192, 168, 0, 1))],
        vec![
            (
                Ipv4Addr::new(192, 168, 0, 0),
                Ipv4Addr::new(255, 255, 0, 0),
                Ipv4Addr::new(192, 168, 0, 1),
            ),
            (
                Ipv4Addr::new(0, 0, 0, 0),
                Ipv4Addr::new(0, 0, 0, 0),
                Ipv4Addr::new(192, 168, 0, 1),
            ),
        ],
    );
    let server = builder.finish_with_channel(channel).expect("Server creating error");
    async move {
        if let Err(e) = server.await {
            error!("DHCP server terminated with {:?}", e);
        }
    }
}


#[tokio::main]
async fn main() {
    std::env::set_var("RUST_BACKTRACE", "1");
    std::env::set_var("RUST_LOG", "info,client=trace,dhcp_client=trace");
    env_logger::init();

    let mut first = true;
    let (client_chn, server_chn) = unreliable_channel(move |msg| {
        let msg = &msg.1;
        info!("Filter: {:?}", msg.options.dhcp_message_type);
        if msg.options.dhcp_message_type == Some(MessageType::DhcpRequest) {
            info!("Filter: DHCPREQUEST");
            if first {
                info!("Filtering request");
                first = false;
                false
            } else {
                true
            }
        } else {
            true
        }
    });
    let client = client(client_chn);
    let server = server(server_chn);
    tokio::spawn(server);
    client.await
}
