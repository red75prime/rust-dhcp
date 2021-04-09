use std::task::Poll;
use std::net::SocketAddrV4;
use std::io;
use tokio::runtime::Handle;

pub struct RawUdpSocketV4 {}

impl RawUdpSocketV4 {
    pub fn new(
        _iface: &str,
        _port: u16,
        _max_packet_size: usize,
        _handle: &Handle,
    ) -> Result<RawUdpSocketV4, io::Error> {
        unimplemented!()
    }

    pub fn poll_recv_from(&mut self, _buf: &mut [u8]) -> Poll<Result<(usize, SocketAddrV4), io::Error>> {
        unimplemented!()
    }

    pub fn poll_send_to(&mut self, _buf: &[u8], _target: &SocketAddrV4) -> Poll<Result<usize, io::Error>> {
        unimplemented!()
    }
}
