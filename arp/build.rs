fn main() {
    #[cfg(windows)]
    windows::build!(
        Windows::Win32::IpHelper::{CreateIpNetEntry, SetIpNetEntry, FreeMibTable, GetIfTable2, MIB_IPNET_TYPE, IP_INTERFACE_INFO, IP_ADAPTER_INDEX_MAP},
        Windows::Win32::Mib::{MIB_IPNETROW_LH, MIB_IF_TABLE2, MIB_IF_ROW2},
        Windows::Win32::SystemServices::{ERROR_INSUFFICIENT_BUFFER, ERROR_NO_DATA, ERROR_INVALID_PARAMETER, NOERROR},
    );
}
