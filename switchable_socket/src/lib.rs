//! A modified version of `tokio::UdpFramed` socket
//! designed to work with high level DHCP messages.

pub mod dummy;
#[cfg(target_os = "linux")]
mod impl_linux;
#[cfg(not(target_os = "linux"))]
mod impl_not_linux;
#[cfg(target_os = "linux")]
pub mod linux;
mod socket;

#[macro_use]
extern crate futures;
#[macro_use]
extern crate log;

#[cfg(target_os = "linux")]
pub use impl_linux::RawUdpSocketV4;
#[cfg(not(target_os = "linux"))]
pub use impl_not_linux::RawUdpSocketV4;

pub use socket::{MakeSocket, ModeSwitch, SocketMode, SwitchableUdpSocket, UdpAsyncReadWrite};
