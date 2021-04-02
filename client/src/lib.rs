//! The original Rust DHCP client implementation.

#[macro_use]
mod macros;
mod backoff;
mod builder;
mod client;
mod forthon;
mod state;

#[macro_use]
extern crate log;
extern crate tokio;
#[macro_use]
extern crate futures;
extern crate bytes;
extern crate eui48;
extern crate hostname;
extern crate pin_project;
extern crate rand;

extern crate dhcp_framed;
extern crate dhcp_protocol;
extern crate switchable_socket;

pub use self::client::{Client, Command, Configuration};
