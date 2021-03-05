use crate::socket::UdpAsyncReadWrite;
use futures::{Async, Poll};
use mio::{event::Evented, unix::EventedFd};
use std::{
    net::{SocketAddr, SocketAddrV4},
    os::unix::io::RawFd,
};
use tokio::{
    io,
    reactor::{Handle, PollEvented2},
};

const IPV4_HEADER_SIZE_MAX: usize = 60;
const IPV4_HEADER_SIZE_MIN: usize = 20;
const UDP_HEADER_SIZE: usize = 8;
// Value from IPDEFTTL in linux/ip.h
const DHCP_DEF_TTL: u8 = 64;
// TODO: libc for uclibc lacks `ETH_P_IP` and other constants
const ETH_P_IP: libc::c_int = 0x0800;

pub struct RawUdpSocketV4 {
    io: PollEvented2<RawMioSocket>,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
    port: u16,
}

impl RawUdpSocketV4 {
    pub fn new(
        iface: &str,
        port: u16,
        max_packet_size: usize,
        handle: &Handle,
    ) -> Result<RawUdpSocketV4, io::Error> {
        let raw_mio_socket = RawMioSocket::new(iface)?;
        let io = PollEvented2::new_with_handle(raw_mio_socket, handle)?;
        // TODO: something with allocations
        let adjusted_buf_len = max_packet_size + IPV4_HEADER_SIZE_MAX + UDP_HEADER_SIZE;
        let read_buf = vec![0; adjusted_buf_len];
        let write_buf = vec![0; adjusted_buf_len];
        Ok(RawUdpSocketV4 {
            io,
            read_buf,
            write_buf,
            port,
        })
    }

    pub fn poll_recv_from_v4(&mut self, buf: &mut [u8]) -> Poll<(usize, SocketAddrV4), io::Error> {
        use etherparse::{InternetSlice, SlicedPacket, TransportSlice};

        try_ready!(self.io.poll_read_ready(mio::Ready::readable()));

        let fd: RawFd = self.io.get_ref().io;
        // TODO: Packets with uncomputed checksum
        let result = read_fd(fd, &mut self.read_buf);
        let n = match result {
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.io.clear_read_ready(mio::Ready::readable())?;
                return Ok(Async::NotReady);
            }
            Err(e) => {
                return Err(e);
            }
        };
        if n < IPV4_HEADER_SIZE_MIN + UDP_HEADER_SIZE {
            // Ignore. It's not an UDP packet
            self.io.clear_read_ready(mio::Ready::readable())?;
            return Ok(Async::NotReady);
        }
        match &SlicedPacket::from_ip(&self.read_buf[..n]) {
            Ok(SlicedPacket {
                ip: Some(InternetSlice::Ipv4(ipv4)),
                transport: Some(TransportSlice::Udp(udp)),
                payload,
                ..
            }) => {
                if udp.destination_port() == self.port {
                    let payload_len = std::cmp::min(payload.len(), buf.len());
                    buf[..payload_len].copy_from_slice(&payload[..payload_len]);
                    let src_addr = SocketAddrV4::new(ipv4.source_addr(), udp.source_port());
                    return Ok(Async::Ready((payload_len, src_addr)));
                } else {
                    // ignore
                    if log_enabled!(log::Level::Trace) {
                        trace!("Ignoring (wrong port {}) {:02x?} {:02x?}", udp.destination_port(), ipv4, udp);
                    }
                }
            }
            Ok(packet) => {
                if log_enabled!(log::Level::Trace) {
                    trace!("Ignoring {:02x?}", packet);
                }
            }
            _ => {
                // Ignore
                trace!("Ignoring ill-formed packet");
            }
        };
        self.io.clear_read_ready(mio::Ready::readable())?;
        Ok(Async::NotReady)
    }

    pub fn poll_send_to_v4(&mut self, buf: &[u8], target: &SocketAddrV4) -> Poll<usize, io::Error> {
        use etherparse::PacketBuilder;

        try_ready!(self.io.poll_write_ready());

        let builder = PacketBuilder::ipv4([0, 0, 0, 0], target.ip().octets(), DHCP_DEF_TTL)
            .udp(self.port, target.port());
        let packet_len = builder.size(buf.len());
        let header_len = packet_len - buf.len();
        let mut write_buf = &mut self.write_buf[..];
        if let Err(_) = builder.write(&mut write_buf, buf) {
            self.io.clear_write_ready()?;
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Packet too big"));
        }

        let fd: RawFd = self.io.get_ref().io;
        let ifindex = self.io.get_ref().ifindex;
        let sockaddr = libc::sockaddr_ll {
            sll_family: libc::AF_PACKET as u16,
            sll_protocol: (self::ETH_P_IP as u16).to_be(),
            sll_ifindex: ifindex,
            sll_hatype: 0,
            sll_pkttype: 0,
            sll_halen: 0,
            // MAC broadcast
            sll_addr: [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0],
        };
        let result = unsafe {
            libc::sendto(
                fd,
                self.write_buf.as_ptr() as *const libc::c_void,
                packet_len,
                0,
                &sockaddr as *const _ as *const libc::sockaddr,
                std::mem::size_of_val(&sockaddr) as u32,
            )
        };
        if result < 0 {
            self.io.clear_write_ready()?;
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                return Ok(Async::NotReady);
            } else {
                return Err(err);
            }
        };
        let result = result as usize;
        let payload_sent_len =
            if result >= header_len {
                result - header_len
            } else {
                0
            };
        Ok(Async::Ready(payload_sent_len))
    }
}

impl UdpAsyncReadWrite for RawUdpSocketV4 {
    fn poll_recv_from(&mut self, buf: &mut [u8]) -> Poll<(usize, SocketAddr), io::Error> {
        let (len, addr) = try_ready!(self.poll_recv_from_v4(buf));
        Ok(Async::Ready((len, SocketAddr::V4(addr))))
    }

    fn poll_send_to(&mut self, buf: &[u8], target: &SocketAddr) -> Poll<usize, io::Error> {
        match target {
            SocketAddr::V4(target) => self.poll_send_to_v4(buf, target),
            SocketAddr::V6(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "IPV6 isn't supported",
            )),
        }
    }
}

const SIOCGIFINDEX: u32 = 0x8933;
const IFNAMSIZ: usize = 16;

type IfrnName = [libc::c_char; IFNAMSIZ];

#[repr(C)]
struct IfreqIndex {
    ifrn_name: IfrnName,
    ifindex: libc::c_int,
}

impl Default for IfreqIndex {
    fn default() -> Self {
        IfreqIndex {
            ifrn_name: [0; IFNAMSIZ],
            ifindex: 0,
        }
    }
}

fn ifindex(iface: &str) -> Result<libc::c_int, io::Error> {
    use libc::*;

    let mut ifreq = IfreqIndex::default();
    if iface.len() > IFNAMSIZ {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "iface name too long",
        ));
    }
    for (dst, src) in ifreq.ifrn_name.iter_mut().zip(iface.bytes()) {
        *dst = src as c_char;
    }
    unsafe {
        let fd = socket(AF_INET, SOCK_RAW, IPPROTO_RAW);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let code = ioctl(fd, SIOCGIFINDEX.into(), &mut ifreq);
        if code != 0 {
            close(fd);
            return Err(io::Error::last_os_error());
        }
        close(fd);
    }
    Ok(ifreq.ifindex)
}

struct RawMioSocket {
    io: RawFd,
    ifindex: libc::c_int,
}

impl RawMioSocket {
    fn new(iface: &str) -> Result<RawMioSocket, io::Error> {
        use libc::*;
        use std::mem;

        let ifindex = ifindex(iface)?;
        unsafe {
            let fd = socket(PF_PACKET, SOCK_DGRAM, self::ETH_P_IP.to_be());
            // PF_PACKET sockets cannot be bound to device by using setsockopt
            let mut sockaddr = sockaddr_ll {
                sll_family: AF_PACKET as u16,
                sll_protocol: (self::ETH_P_IP as u16).to_be(),
                sll_ifindex: ifindex,
                sll_hatype: 0,
                sll_pkttype: 0,
                sll_halen: 0,
                sll_addr: [0; 8],
            };
            if bind(
                fd,
                &mut sockaddr as *const _ as *const _,
                mem::size_of_val(&sockaddr) as libc::socklen_t,
            ) < 0
            {
                return Err(io::Error::last_os_error());
            }
            let flags = fcntl(fd, F_GETFL);
            if fcntl(fd, F_SETFD, flags | O_NONBLOCK) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(RawMioSocket { io: fd, ifindex })
        }
    }
}

impl Drop for RawMioSocket {
    fn drop(&mut self) {
        let ret = unsafe { libc::close(self.io) };
        if ret != 0 {
            error!("RawMioSocket::drop(): {:?}", io::Error::last_os_error());
        }
    }
}

impl Evented for RawMioSocket {
    fn register(
        &self,
        poll: &mio::Poll,
        token: mio::Token,
        interest: mio::Ready,
        opts: mio::PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.io).register(poll, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &mio::Poll,
        token: mio::Token,
        interest: mio::Ready,
        opts: mio::PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.io).reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &mio::Poll) -> io::Result<()> {
        EventedFd(&self.io).deregister(poll)
    }
}

fn read_fd(fd: RawFd, buf: &mut [u8]) -> Result<usize, io::Error> {
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n < 0 {
            if unsafe { *libc::__errno_location() == libc::EINTR } {
                continue;
            }
            break Err(io::Error::last_os_error());
        } else {
            break Ok(n as usize);
        }
    }
}
