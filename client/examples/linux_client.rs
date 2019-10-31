//! Run this with administrator privileges where it is required
//! Works only under linux
//! in order to bind the DHCP client socket to its port 68.

#[macro_use]
extern crate log;
extern crate tokio;
#[macro_use]
extern crate futures;
extern crate env_logger;
extern crate eui48;
extern crate rand;

extern crate dhcp_client;
extern crate dhcp_framed;
extern crate dhcp_protocol;
extern crate switchable_socket;

extern crate net2;

use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

use eui48::MacAddress;
use tokio::prelude::*;
use tokio::reactor::Handle;

use dhcp_client::{Client, Command};
use dhcp_framed::{
    DhcpFramed, DhcpSinkItem, DhcpStreamItem, BUFFER_READ_CAPACITY, BUFFER_WRITE_CAPACITY,
};
use dhcp_protocol::{DHCP_PORT_CLIENT, SIZE_MESSAGE_MINIMAL};
use net2::UdpBuilder;
#[cfg(not(target_os = "linux"))]
use switchable_socket::dummy;
#[cfg(target_os = "linux")]
use switchable_socket::linux;
use switchable_socket::ModeSwitch;
use tokio::net::UdpSocket;

struct SuperClient<IO> {
    inner: Client<IO>,
    counter: u64,
}

impl<IO> SuperClient<IO>
where
    IO: Stream<Item = DhcpStreamItem, Error = io::Error>
        + Sink<SinkItem = DhcpSinkItem, SinkError = io::Error>
        + ModeSwitch
        + Send
        + Sync,
{
    pub fn new(client: Client<IO>) -> Self {
        SuperClient {
            inner: client,
            counter: 0,
        }
    }
}

impl<IO> Future for SuperClient<IO>
where
    IO: Stream<Item = DhcpStreamItem, Error = io::Error>
        + Sink<SinkItem = DhcpSinkItem, SinkError = io::Error>
        + ModeSwitch
        + Send
        + Sync,
{
    type Item = ();
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        loop {
            let result =
                try_ready!(self.inner.poll()).expect("The client returned None but it must not");
            info!("{:?}", result);
            self.counter += 1;
            if self.counter >= 5 {
                self.inner.start_send(Command::Release { message: None })?;
                self.inner.poll_complete()?;
                break;
            }
        }
        Ok(Async::Ready(()))
    }
}

fn main() {
    std::env::set_var("RUST_BACKTRACE", "1");
    std::env::set_var("RUST_LOG", "client=trace,dhcp_client=trace");
    env_logger::init();

    let iface_str = "ens33";

    let server_address = None;
    let client_address = None;
    let address_request = Some(Ipv4Addr::new(192, 168, 0, 60));
    let address_time = Some(60);
    let max_message_size = Some(SIZE_MESSAGE_MINIMAL as u16);

    #[cfg(target_os = "linux")]
    let switchable_socket = {
        let buffer_capacity = std::cmp::max(BUFFER_WRITE_CAPACITY, BUFFER_READ_CAPACITY);
        linux::switchable_udp_socket(iface_str, DHCP_PORT_CLIENT, buffer_capacity)
            .expect("Cannot create switchable socket")
    };
    #[cfg(not(target_os = "linux"))]
    let switchable_socket = dummy::switchable_udp_socket(iface_str, DHCP_PORT_CLIENT)
        .expect("Cannot create switchable socket");

    let dhcp_framed = DhcpFramed::new(switchable_socket).expect("Cannot create DhcpFramed");

    let client = SuperClient::new(Client::new(
        dhcp_framed,
        MacAddress::new([0x00, 0x0c, 0x29, 0x13, 0x0e, 0x37]),
        None,
        None,
        server_address,
        client_address,
        address_request,
        address_time,
        max_message_size,
    ));

    let future = client.map_err(|error| error!("Error: {}", error));

    info!("DHCP client started");
    tokio::run(future);
}
