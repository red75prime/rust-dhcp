//! A builder for common DHCP server messages.

use std::{
    net::{
        Ipv4Addr,
    },
};

use dhcp_protocol::*;

use database::{
    Offer,
    Ack,
    Error,
};

/// Builds common server messages with some parameters.
pub struct MessageBuilder {
    /// Sent to clients in `server_ip_address` field.
    server_ip_address       : Ipv4Addr,
    /// Sent to clients in `hostname` option.
    hostname                : Option<String>,
    /// Sent to clients in options.
    subnet_mask             : Ipv4Addr,
    /// Sent to clients in options.
    routers                 : Vec<Ipv4Addr>,
    /// Sent to clients in options.
    domain_name_servers     : Vec<Ipv4Addr>,
    /// Sent to clients in options.
    static_routes           : Vec<(Ipv4Addr, Ipv4Addr)>,
}

impl MessageBuilder {
    /// Creates a builder with message parameters which will not be changed.
    pub fn new(
        server_ip_address       : Ipv4Addr,
        hostname                : Option<String>,

        subnet_mask             : Ipv4Addr,
        routers                 : Vec<Ipv4Addr>,
        domain_name_servers     : Vec<Ipv4Addr>,
        static_routes           : Vec<(Ipv4Addr, Ipv4Addr)>,
    ) -> Self {
        MessageBuilder {
            server_ip_address,
            hostname,

            subnet_mask,
            routers,
            domain_name_servers,
            static_routes,
        }
    }

    /// Creates a `DHCPOFFER` message from a `DHCPDISCOVER` message.
    pub fn dhcp_discover_to_offer(
        &self,
        discover                        : &Message,
        offer                           : &Offer,
    ) -> Message {
        let mut options = Options::default();
        self.append_default_options(&mut options);
        if let Some(ref parameter_list) = discover.options.parameter_list {
            self.append_requested_options(&mut options, parameter_list);
        }

        options.dhcp_message_type       = Some(MessageType::DhcpOffer);
        options.dhcp_message            = Some(offer.message.to_owned());
        options.address_time            = Some(offer.lease_time);

        Message {
            operation_code              : OperationCode::BootReply,
            hardware_type               : HardwareType::Ethernet,
            hardware_address_length     : discover.hardware_address_length,
            hardware_options            : 0u8,

            transaction_id              : discover.transaction_id,
            seconds                     : 0u16,
            is_broadcast                : discover.is_broadcast,

            client_ip_address           : Ipv4Addr::new(0,0,0,0),
            your_ip_address             : offer.address,
            server_ip_address           : Ipv4Addr::new(0,0,0,0),
            gateway_ip_address          : discover.gateway_ip_address,

            client_hardware_address     : discover.client_hardware_address,
            server_name                 : String::new(),
            boot_filename               : String::new(),

            options,
        }
    }

    /// Creates a `DHCPACK` message from a `DHCPREQUEST` message.
    pub fn dhcp_request_to_ack(
        &self,
        request                         : &Message,
        ack                             : &Ack,
    ) -> Message {
        let mut options = Options::default();
        self.append_default_options(&mut options);
        if let Some(ref parameter_list) = request.options.parameter_list {
            self.append_requested_options(&mut options, parameter_list);
        }

        options.dhcp_message_type       = Some(MessageType::DhcpAck);
        options.dhcp_message            = Some(ack.message.to_owned());
        options.address_time            = Some(ack.lease_time);
        options.renewal_time            = Some(ack.renewal_time);
        options.rebinding_time          = Some(ack.rebinding_time);

        Message {
            operation_code              : OperationCode::BootReply,
            hardware_type               : HardwareType::Ethernet,
            hardware_address_length     : request.hardware_address_length,
            hardware_options            : 0u8,

            transaction_id              : request.transaction_id,
            seconds                     : 0u16,
            is_broadcast                : request.is_broadcast,

            client_ip_address           : request.client_ip_address,
            your_ip_address             : ack.address,
            server_ip_address           : self.server_ip_address,
            gateway_ip_address          : request.gateway_ip_address,

            client_hardware_address     : request.client_hardware_address,
            server_name                 : String::new(),
            boot_filename               : String::new(),

            options,
        }
    }

    /// Creates a `DHCPACK` message from a `DHCPINFORM` message.
    pub fn dhcp_inform_to_ack(
        &self,
        inform                          : &Message,
        message                         : &str,
    ) -> Message {
        let mut options = Options::default();
        self.append_default_options(&mut options);
        if let Some(ref parameter_list) = inform.options.parameter_list {
            self.append_requested_options(&mut options, parameter_list);
        }

        options.dhcp_message_type       = Some(MessageType::DhcpAck);
        options.dhcp_message            = Some(message.to_owned());

        Message {
            operation_code              : OperationCode::BootReply,
            hardware_type               : HardwareType::Ethernet,
            hardware_address_length     : inform.hardware_address_length,
            hardware_options            : 0u8,

            transaction_id              : inform.transaction_id,
            seconds                     : 0u16,
            is_broadcast                : inform.is_broadcast,

            client_ip_address           : inform.client_ip_address,
            your_ip_address             : Ipv4Addr::new(0,0,0,0),
            server_ip_address           : self.server_ip_address,
            gateway_ip_address          : inform.gateway_ip_address,

            client_hardware_address     : inform.client_hardware_address,
            server_name                 : String::new(),
            boot_filename               : String::new(),

            options,
        }
    }

    /// Creates a `DHCPNAK` message from a `DHCPREQUEST` message.
    pub fn dhcp_request_to_nak(
        &self,
        request                         : &Message,
        error                           : &Error,
    ) -> Message {
        let mut options = Options::default();
        self.append_default_options(&mut options);

        options.dhcp_message_type       = Some(MessageType::DhcpNak);
        options.dhcp_message            = Some(error.to_string());

        Message {
            operation_code              : OperationCode::BootReply,
            hardware_type               : HardwareType::Ethernet,
            hardware_address_length     : request.hardware_address_length,
            hardware_options            : 0u8,

            transaction_id              : request.transaction_id,
            seconds                     : 0u16,
            is_broadcast                : request.is_broadcast,

            client_ip_address           : Ipv4Addr::new(0,0,0,0),
            your_ip_address             : Ipv4Addr::new(0,0,0,0),
            server_ip_address           : Ipv4Addr::new(0,0,0,0),
            gateway_ip_address          : request.gateway_ip_address,

            client_hardware_address     : request.client_hardware_address,
            server_name                 : String::new(),
            boot_filename               : String::new(),

            options,
        }
    }

    fn append_default_options(&self, options: &mut Options) {
        options.hostname = self.hostname.to_owned();
        options.dhcp_server_id = Some(self.server_ip_address);
    }

    fn append_requested_options(&self, options: &mut Options, parameter_list: &[u8]) {
        for tag in parameter_list {
            match (*tag).into() {
                OptionTag::SubnetMask => options.subnet_mask = Some(self.subnet_mask),
                OptionTag::Routers => if self.routers.len() > 0 {
                    options.routers = Some(self.routers.to_owned());
                },
                OptionTag::DomainNameServers => if self.domain_name_servers.len() > 0 {
                    options.domain_name_servers = Some(self.domain_name_servers.to_owned());
                },
                OptionTag::StaticRoutes => if self.static_routes.len() > 0 {
                    options.static_routes = Some(self.static_routes.to_owned())
                },
                _ => continue,
            }
        }
    }
}