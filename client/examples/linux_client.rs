//! Run this with administrator privileges where it is required
//! Works only under linux
//! in order to bind the DHCP client socket to its port 68.

#[macro_use]
extern crate log;

use std::{io, net::Ipv4Addr};

use eui48::MacAddress;
use futures::{Sink, SinkExt, Stream, StreamExt};

use dhcp_client::{Client, Command};
use dhcp_framed::{
    DhcpFramed, DhcpSinkItem, DhcpStreamItem,
};
use dhcp_protocol::{DHCP_PORT_CLIENT, SIZE_MESSAGE_MINIMAL};
#[cfg(not(target_os = "linux"))]
use switchable_socket::dummy;
#[cfg(target_os = "linux")]
use switchable_socket::linux;
use switchable_socket::ModeSwitch;

async fn super_client<IO>(client: Client<IO>) -> Result<(), io::Error>
where
    IO: Stream<Item = Result<DhcpStreamItem, io::Error>>
    + Sink<DhcpSinkItem, Error = io::Error>
    + ModeSwitch
    + Send
    + Sync,
{
    futures::pin_mut!(client);
    let mut counter = 0;
    loop {
        let result = client.next().await.expect("The client returned None but it must not");
        info!("{:?}", result);
        counter += 1;
        if counter >= 5 {
            client.send(Command::Release { message: None}).await?;
        }
    }
}

fn main() {
    std::env::set_var("RUST_BACKTRACE", "1");
    std::env::set_var("RUST_LOG", "trace,tokio=trace,client=trace,dhcp_client=trace");
    env_logger::init();

    let iface_str = "eth0";

    let server_address = None;
    let client_address = None;
    let address_request = Some(Ipv4Addr::new(192, 168, 0, 60));
    let address_time = Some(60);
    let max_message_size = Some(SIZE_MESSAGE_MINIMAL as u16);


    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("Cannot create tokio runtime");
    info!("DHCP client started");
    rt.block_on(async move {
        #[cfg(target_os = "linux")]
        let switchable_socket = {
            let buffer_capacity = std::cmp::max(dhcp_framed::BUFFER_WRITE_CAPACITY, dhcp_framed::BUFFER_READ_CAPACITY);
            linux::switchable_udp_socket(iface_str, DHCP_PORT_CLIENT, buffer_capacity)
                .expect("Cannot create switchable socket")
        };
        #[cfg(not(target_os = "linux"))]
        let switchable_socket = dummy::switchable_udp_socket(iface_str, DHCP_PORT_CLIENT)
            .expect("Cannot create switchable socket");

        let dhcp_framed = DhcpFramed::new(switchable_socket).expect("Cannot create DhcpFramed");

        let request_static_routes = false;

        let client = super_client(Client::new(
            dhcp_framed,
            MacAddress::new([0x00, 0x0c, 0x29, 0x13, 0x0e, 0x37]),
            None,
            None,
            server_address,
            client_address,
            address_request,
            address_time,
            max_message_size,
            request_static_routes,
        ));

        let result = client.await;
        info!("Result: {:?}", result);
    });
}
