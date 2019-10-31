use crate::socket::{MakeSocket, SocketMode, SwitchableUdpSocket};
use crate::RawUdpSocketV4;
use libc;
use net2::UdpBuilder;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    os::unix::io::{AsRawFd, RawFd},
};
use tokio::{io, net::UdpSocket, reactor::Handle};

pub struct MakeRaw {
    pub max_packet_size: usize,
}

impl MakeSocket for MakeRaw {
    type Socket = RawUdpSocketV4;
    fn make(&mut self, iface: &str, port: u16) -> Result<RawUdpSocketV4, io::Error> {
        RawUdpSocketV4::new(iface, port, self.max_packet_size, &Handle::default())
    }
}

pub struct MakeUdp;

impl MakeSocket for MakeUdp {
    type Socket = UdpSocket;
    fn make(&mut self, iface: &str, port: u16) -> Result<UdpSocket, io::Error> {
        let socket = UdpBuilder::new_v4()?;
        socket.reuse_address(true)?;
        let socket = socket.bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port))?;
        // Bind to interface
        bind_to_device_raw(socket.as_raw_fd(), iface)?;
        let socket = UdpSocket::from_std(socket, &Handle::default())?;
        socket.set_broadcast(true)?;
        Ok(socket)
    }
}

pub fn bind_to_device_raw(socket: RawFd, iface_name: &str) -> Result<(), io::Error> {
    let cstr = std::ffi::CString::new(iface_name).unwrap();
    let res = unsafe {
        libc::setsockopt(
            socket,
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            cstr.as_bytes().as_ptr() as *const _ as *const _,
            iface_name.len() as u32,
        )
    };
    if res != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub type LinuxSwitchableUdpSocket =
    SwitchableUdpSocket<RawUdpSocketV4, UdpSocket, MakeRaw, MakeUdp>;

pub fn switchable_udp_socket(
    iface: &str,
    port: u16,
    max_packet_size: usize,
) -> Result<LinuxSwitchableUdpSocket, io::Error> {
    SwitchableUdpSocket::new(
        iface,
        port,
        SocketMode::Raw,
        MakeRaw { max_packet_size },
        MakeUdp,
    )
}
