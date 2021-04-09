//! Run this with administrator privileges where it is required
//! in order to bind the DHCP server socket to its port 67 or use other OS-specific features.

#[macro_use]
extern crate log;
extern crate env_logger;
extern crate tokio;

extern crate dhcp_protocol;
extern crate dhcp_server;

use std::net::Ipv4Addr;

use dhcp_protocol::DHCP_PORT_SERVER;

#[tokio::main]
async fn main() {
    std::env::set_var("RUST_BACKTRACE", "full");
    std::env::set_var("RUST_LOG", "server=trace,dhcp_server=trace,dhcp_arp=trace");
    env_logger::init();
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
    #[cfg(any(target_os = "freebsd", target_os = "macos"))]
    {
        builder.with_bpf_num_threads(8);
    }
    let server = builder.finish().await.expect("Server creating error");

    info!(
        "DHCP server started on {}:{}",
        server_ip_address, DHCP_PORT_SERVER
    );
    if let Err(e) = server.await {
        error!("DHCP server error: {:?}", e);
    }
}
