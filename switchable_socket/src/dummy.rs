use crate::socket::{SwitchableUdpSocket, MakeSocket, SocketMode};
use net2::UdpBuilder;
use std::{
    net::{SocketAddr, IpAddr, Ipv4Addr},
};
use tokio::{
    io,
    net::UdpSocket,
    reactor::Handle,
};

pub struct MakeUdp;

impl MakeSocket for MakeUdp {
    type Socket = UdpSocket;
    fn make(&mut self, _iface: &str, port: u16) -> Result<UdpSocket, io::Error> {
        let socket = UdpBuilder::new_v4()?;
        socket.reuse_address(true)?;
        let socket = socket
            .bind(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
                port,
            ))?;
        // Bind to interface
        let socket = UdpSocket::from_std(socket, &Handle::default())?;
        socket.set_broadcast(true)?;
        Ok(socket)
    }
}

pub type DummySwitchableUdpSocket = SwitchableUdpSocket<UdpSocket, UdpSocket, MakeUdp, MakeUdp>;

pub fn switchable_udp_socket(iface: &str, port: u16) -> Result<DummySwitchableUdpSocket, io::Error> {
    SwitchableUdpSocket::new(iface, port, SocketMode::Raw, MakeUdp, MakeUdp)
}
