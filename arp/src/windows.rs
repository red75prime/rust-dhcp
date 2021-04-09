//! The Windows implementation using CreateIpNetEntry

use core::slice;
use std::{ffi::{OsString, c_void}, net::Ipv4Addr};

use eui48::MacAddress;
pub use windows::{Error, ErrorCode};

mod bindings {
    windows::include_bindings!();
}

use bindings::{
    Windows::Win32::IpHelper::{
        CreateIpNetEntry, SetIpNetEntry, GetIfTable2, FreeMibTable,
        MIB_IPNET_TYPE, IF_OPER_STATUS,
    },
    Windows::Win32::Mib::{MIB_IPNETROW_LH, MIB_IPNETROW_LH_0, MIB_IF_TABLE2, MIB_IF_ROW2},
    Windows::Win32::NetworkDrivers::NET_IF_MEDIA_CONNECT_STATE,
    Windows::Win32::SystemServices::{
        ERROR_INVALID_PARAMETER, ERROR_NO_DATA, NOERROR,
    },
};

struct AllocGuard(*mut MIB_IF_TABLE2);

impl AllocGuard {
    // Safety: self.0 should point to initialized memory with correct size
    unsafe fn index_map(&self) -> &[MIB_IF_ROW2] {
        let r = self.0.as_ref().unwrap();
        let len = r.NumEntries as usize;
        slice::from_raw_parts(r.Table.as_ptr(), len)
    }
}

impl Drop for AllocGuard {
    fn drop(&mut self) {
        unsafe {
            FreeMibTable(self.0 as *mut c_void);
        }
    }
}

fn os_str_from_wide(s: &[u16]) -> OsString {
    use std::os::windows::ffi::OsStringExt;

    let len = s.iter().position(|&w| w == 0).unwrap_or(s.len());
    OsStringExt::from_wide(&s[..len])
}

fn if_index(iface_name: &str) -> Result<u32, Error> {
    let mut table_ptr = std::ptr::null_mut();
    unsafe {
        let result = GetIfTable2(&mut table_ptr);
        if result.0 == 0 {
            // proceed
        } else {
            return Err(Error::new(
                // TODO: is it the right thing?
                ErrorCode::from_win32(result.0 as u32),
                "No active network interfaces",
            ));
        };
        // Safety: memory is initialized by GetIfTable2
        let ifaces = AllocGuard(table_ptr);
        let index_map = ifaces.index_map();
        for iface in index_map {
            if iface.OperStatus != IF_OPER_STATUS::IfOperStatusUp
                || iface.MediaConnectState != NET_IF_MEDIA_CONNECT_STATE::MediaConnectStateConnected
                // exclude filtering interfaces
                || iface.InterfaceAndOperStatusFlags._bitfield & 2 != 0 {
                continue;
            };
            let cur_iface_name = os_str_from_wide(&iface.Alias);
            log::trace!("Network interface #{}: {:?}", iface.InterfaceIndex, cur_iface_name);
            if &cur_iface_name == iface_name {
                return Ok(iface.InterfaceIndex);
            }
        }
    }
    return Err(Error::new(
        ErrorCode::from_win32(ERROR_NO_DATA),
        "Network interface wasn't found",
    ));
}

pub(crate) fn add(hwaddr: MacAddress, ip: Ipv4Addr, iface: String) -> Result<super::Arp, Error> {
    let a = hwaddr.to_array();
    let mut entry = MIB_IPNETROW_LH {
        dwIndex: if_index(&iface)?,
        dwPhysAddrLen: 6,
        bPhysAddr: [a[0], a[1], a[2], a[3], a[4], a[5], 0, 0],
        dwAddr: u32::from_be_bytes(ip.octets()),
        Anonymous: MIB_IPNETROW_LH_0 {
            Type: MIB_IPNET_TYPE::MIB_IPNET_TYPE_DYNAMIC,
        },
    };
    unsafe {
        let mut result = CreateIpNetEntry(&mut entry);
        if result == ERROR_INVALID_PARAMETER {
            result = SetIpNetEntry(&mut entry);
        }
        if result == NOERROR {
            Ok(())
        } else {
            Err(Error::new(
                ErrorCode::from_win32(result),
                "Cannot create or set ip net entry",
            ))
        }
    }
}
