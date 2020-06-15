#[macro_use]
extern crate log;
#[macro_use]
extern crate futures;

use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
};

use eui48::MacAddress;
use tokio::prelude::*;

use dhcp_client::{Client, Command};
use dhcp_server::GenericServer;
use dhcp_framed::{
    DhcpSinkItem, DhcpStreamItem
};
use dhcp_protocol::{MessageType};
use switchable_socket::{ModeSwitch, SocketMode};
use futures::sync::mpsc::{self, Sender, Receiver};
use futures::{Poll, Async, AsyncSink, StartSend};

struct UnreliableChannel<F> {
    filter: F,
    mode: SocketMode,
    sender: Sender<DhcpStreamItem>,
    receiver: Receiver<DhcpStreamItem>,
}

impl<F> Stream for UnreliableChannel<F>
where
    F: 'static + Send + Sync + FnMut(&DhcpStreamItem) -> bool,
{
    type Item = DhcpStreamItem;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let item = match self.receiver.poll() {
            Ok(Async::NotReady) => {
                return Ok(Async::NotReady);
            }
            Ok(Async::Ready(item)) => {
                item
            }
            Err(_) => {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Channel closed"));
            }
        };
        if let Some(item) = item {
            info!("Filtering {:?}", item.1.options.dhcp_message_type);
            if !(self.filter)(&item) {
                warn!("Dropping {}", item.1);
                Ok(Async::NotReady)
            } else {
                Ok(Async::Ready(Some(item)))
            }
        } else {
            Ok(Async::Ready(None))
        }
    }
}

impl<F> Sink for UnreliableChannel<F>
where
    F: 'static + Send + Sync + FnMut(&DhcpStreamItem) -> bool,
{
    type SinkItem = DhcpSinkItem;
    type SinkError = io::Error;

    fn start_send(&mut self, item: Self::SinkItem) -> StartSend<Self::SinkItem, Self::SinkError> {
        let (socket, (msg, size)) = item;
        let si = (socket, msg);
        if !(self.filter)(&si) {
            warn!("Dropping {}", si.1);
            return Ok(AsyncSink::Ready);
        }
        match self.sender.start_send(si) {
            Ok(AsyncSink::NotReady((socket, msg))) => {
                Ok(AsyncSink::NotReady((socket, (msg, size))))
            }
            Ok(AsyncSink::Ready) => {
                Ok(AsyncSink::Ready)
            }
            Err(_) => {
                Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Channel closed"))
            }
        }
    }

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        self.sender.poll_complete().map_err(|_| {
            io::Error::new(io::ErrorKind::UnexpectedEof, "Channel closed")
        })
    }
}

impl<F> ModeSwitch for UnreliableChannel<F> {
    fn switch_to(&mut self, mode: SocketMode) -> Result<(), io::Error> {
        self.mode = mode;
        Ok(())
    }

    fn mode(&self) -> SocketMode {
        self.mode
    }

    fn device_name(&self) -> &str {
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

fn client<F>(channel: UnreliableChannel<F>) -> impl Future<Item = (), Error = ()>
where
    F: 'static + Send + Sync + FnMut(&DhcpStreamItem) -> bool,
{

    let iface_str = "ens33";

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

    let future = client
        .map_err(|error| error!("Error: {}", error))
        .for_each(|msg| {
            info!("Client: {:?}", msg);
            Ok(())
        });

    future
}

fn server<F>(channel: UnreliableChannel<F>) -> impl Future<Item = (), Error = ()>
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
    let future = server.map_err(|error| error!("Error: {}", error));
    future
}


fn main() {
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
    tokio::run(future::lazy(move || {
        tokio::spawn(server);
        client
    }));
}
