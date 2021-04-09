//! The original Rust DHCP server implementation.

#[macro_use]
mod macros;
#[cfg(any(target_os = "freebsd", target_os = "macos"))]
mod bpf;
mod builder;
mod database;
mod lease;
mod server;
mod storage;
mod storage_ram;

#[macro_use]
extern crate log;
#[macro_use]
extern crate failure;
#[cfg(any(target_os = "freebsd", target_os = "macos"))]
#[macro_use]
extern crate arrayref;

pub use self::{
    server::{GenericServer, Server, ServerBuilder},
    storage::Storage,
    storage_ram::RamStorage,
};
