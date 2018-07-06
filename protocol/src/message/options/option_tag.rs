#[allow(dead_code)]
#[derive(Debug)]
pub enum OptionTag {
    // RFC2132
    Pad = 0,
    SubnetMask,
    TimeOffset,
    Router,
    TimeServer,
    NameServer,
    DomainServer,
    LogServer,
    QuotesServer,
    LprServer,
    ImpressServer,
    RlpServer,
    Hostname,
    BootFileSize,
    MeritDumpFile,
    DomainName,
    SwapServer,
    RootPath,
    ExtensionsPath,

    AddressTime = 51,

    MessageType = 53,

    End,
}

impl From<u8> for OptionTag {
    fn from(value: u8) -> Self {
        use self::OptionTag::*;
        match value {
            0 => Pad,
            1 => SubnetMask,
            2 => TimeOffset,
            3 => Router,
            4 => TimeServer,
            5 => NameServer,
            6 => DomainServer,
            7 => LogServer,
            8 => QuotesServer,
            9 => LprServer,
            10 => ImpressServer,
            11 => RlpServer,
            12 => Hostname,
            13 => BootFileSize,
            14 => MeritDumpFile,
            15 => DomainName,
            16 => SwapServer,
            17 => RootPath,
            18 => ExtensionsPath,

            51 => AddressTime,

            53 => MessageType,

            255 => End,
            _ => Pad,
        }
    }
}