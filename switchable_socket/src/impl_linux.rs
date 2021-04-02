use crate::socket::UdpAsyncReadWrite;
use futures::{ready, task::Poll};
use core::{pin::Pin, task::Context};
use std::{net::{SocketAddr, SocketAddrV4}, os::unix::io::RawFd};
use tokio::{
    io,
    io::unix::AsyncFd,
};

const IPV4_HEADER_SIZE_MAX: usize = 60;
const IPV4_HEADER_SIZE_MIN: usize = 20;
const UDP_HEADER_SIZE: usize = 8;
// Value from IPDEFTTL in linux/ip.h
const DHCP_DEF_TTL: u8 = 64;
// TODO: libc for uclibc lacks `ETH_P_IP` and other constants
const ETH_P_IP: libc::c_int = 0x0800;

pub struct RawUdpSocketV4 {
    io: AsyncFd<RawSocket>,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
    port: u16,
}

impl RawUdpSocketV4 {
    pub fn new(
        iface: &str,
        port: u16,
        max_packet_size: usize,
    ) -> Result<RawUdpSocketV4, io::Error> {
        let raw_socket = RawSocket::new(iface)?;
        let io = AsyncFd::new(raw_socket)?;
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

    pub fn poll_recv_from_v4(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<Result<(usize, SocketAddrV4), io::Error>> {
        use etherparse::{InternetSlice, SlicedPacket, TransportSlice};

        let mut read_guard = match ready!(self.io.poll_read_ready(cx)) {
            Ok(g) => g,
            Err(e) => return Poll::Ready(Err(e)),
        };

        // We must ensure that Poll::Pending is returned only when io operation returns WOULDBLOCK
        // try_io takes care of setting waker in that case
        loop {
            // TODO: Packets with uncomputed checksum
            let read_buf = &mut self.read_buf;
            let result = read_guard.try_io(|io| {
                let fd = io.get_ref().fd;
                read_fd(fd, read_buf)
            });
            let n = match result {
                Err(_) => {
                    // Would block
                    return Poll::Pending;
                }
                Ok(Ok(n)) => n,
                Ok(Err(e)) => {
                    return Poll::Ready(Err(e));
                }
            };

            if n < IPV4_HEADER_SIZE_MIN + UDP_HEADER_SIZE {
                // Ignore. It's not an UDP packet
                continue;
            }
            match &mut SlicedPacket::from_ip(&self.read_buf[..n]) {
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
                        return Poll::Ready(Ok((payload_len, src_addr)));
                    } else {
                        // ignore and don't log, too many packets
                        // if log_enabled!(log::Level::Trace) {
                        //     trace!("Ignoring (wrong port {}) {:02x?} {:02x?}", udp.destination_port(), ipv4, udp);
                        // }
                    }
                }
                Ok(packet) => {
                    // ignore and log
                    if log_enabled!(log::Level::Trace) {
                        // don't log payload
                        packet.payload = &[];
                        trace!("Ignoring {:02x?}", packet);
                    }
                }
                _ => {
                    // ignore
                    trace!("Ignoring ill-formed packet");
                }
            };
        } // end loop
    }

    pub fn poll_send_to_v4(&mut self, cx: &mut Context<'_>, buf: &[u8], target: &SocketAddrV4) -> Poll<Result<usize, io::Error>> {
        use etherparse::PacketBuilder;

        let mut write_guard = match ready!(self.io.poll_write_ready(cx)) {
            Ok(g) => g,
            Err(e) => {
                return Poll::Ready(Err(e));
            }
        };

        let builder = PacketBuilder::ipv4([0, 0, 0, 0], target.ip().octets(), DHCP_DEF_TTL)
            .udp(self.port, target.port());
        let packet_len = builder.size(buf.len());
        let header_len = packet_len - buf.len();
        let mut write_buf = &mut self.write_buf[..];
        if let Err(_) = builder.write(&mut write_buf, buf) {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::InvalidData, "Packet too big")));
        }

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
        let result = write_guard.try_io(|io| {
            let result = unsafe {
                libc::sendto(
                    io.get_ref().fd,
                    self.write_buf.as_ptr() as *const libc::c_void,
                    packet_len,
                    0,
                    &sockaddr as *const _ as *const libc::sockaddr,
                    std::mem::size_of_val(&sockaddr) as u32,
                )
            };
            if result < 0 {
                let err = io::Error::last_os_error();
                return Err(err);
            };
            let result = result as usize;
            let payload_sent_len =
                if result >= header_len {
                    result - header_len
                } else {
                    0
                };
            Ok(payload_sent_len)
        });
        match result {
            Err(_) => {
                // would block
                Poll::Pending
            }
            Ok(Ok(len)) => Poll::Ready(Ok(len)),
            Ok(Err(e)) => Poll::Ready(Err(e)),
        }
    }
}

impl UdpAsyncReadWrite for RawUdpSocketV4 {
    fn poll_recv_from(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<Result<(usize, SocketAddr), io::Error>> {
        let this = Pin::into_inner(self);
        match ready!(this.poll_recv_from_v4(cx, buf)) {
            Ok((len, addr)) => Poll::Ready(Ok((len, SocketAddr::V4(addr)))),
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_send_to(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8], target: &SocketAddr) -> Poll<Result<usize, io::Error>> {
        let this = Pin::into_inner(self);
        match target {
            SocketAddr::V4(target) => this.poll_send_to_v4(cx, buf, target),
            SocketAddr::V6(_) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "IPV6 isn't supported",
            ))),
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

struct RawSocket {
    fd: RawFd,
    ifindex: libc::c_int,
}

impl std::os::unix::io::AsRawFd for RawSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl RawSocket {
    fn new(iface: &str) -> Result<RawSocket, io::Error> {
        use libc::*;
        use std::mem;

        let ifindex = ifindex(iface)?;
        unsafe {
            let fd = socket(PF_PACKET, SOCK_DGRAM, self::ETH_P_IP.to_be());
            // PF_PACKET sockets cannot be bound to a device by using setsockopt
            let sockaddr = sockaddr_ll {
                sll_family: AF_PACKET as u16,
                sll_protocol: (self::ETH_P_IP as u16).to_be(),
                sll_ifindex: ifindex,
                sll_hatype: 0,
                sll_pkttype: 0,
                sll_halen: 0,
                sll_addr: [0; 8],
            };
            let sockaddr_ptr = &sockaddr as *const _ as *const sockaddr;
            if bind(fd, sockaddr_ptr, mem::size_of_val(&sockaddr) as libc::socklen_t) < 0
            {
                return Err(io::Error::last_os_error());
            }
            let flags = fcntl(fd, F_GETFL);
            if fcntl(fd, F_SETFD, flags | O_NONBLOCK) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(RawSocket { fd, ifindex })
        }
    }
}

impl Drop for RawSocket {
    fn drop(&mut self) {
        let ret = unsafe { libc::close(self.fd) };
        if ret != 0 {
            error!("RawMioSocket::drop(): {:?}", io::Error::last_os_error());
        }
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
