//! A modified version of `tokio::UdpFramed` socket
//! designed to work with high level DHCP messages.

mod socket;

extern crate tokio;
extern crate futures;
extern crate net2;

extern crate dhcp_protocol;
extern crate switchable_socket;

pub use socket::{
    DhcpFramed, DhcpSinkItem, DhcpStreamItem, BUFFER_READ_CAPACITY, BUFFER_WRITE_CAPACITY,
};
