//! The main DHCP server module.

use std::{convert::Infallible, net::{IpAddr, Ipv4Addr, SocketAddr}};
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::{Future, Stream, Sink};
use hostname;
use tokio::io;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use dhcp_arp;
use dhcp_framed::{DhcpFramed, DhcpStreamItem, DhcpSinkItem};
use dhcp_protocol::{Message, MessageType, DHCP_PORT_CLIENT, DHCP_PORT_SERVER};

#[cfg(any(target_os = "freebsd", target_os = "macos"))]
use bpf::BpfData;
use crate::builder::MessageBuilder;
use crate::database::{Database, Error::LeaseInvalid};
use crate::storage::Storage;
use tokio::net::UdpSocket;
use pin_project::pin_project;

/// Some options like `cpu_pool_size` are OS-specific, so the builder pattern is required.
pub struct ServerBuilder<S>
where
    S: Storage,
{
    server_ip_address: Ipv4Addr,
    iface_name: String,
    static_address_range: (Ipv4Addr, Ipv4Addr),
    dynamic_address_range: (Ipv4Addr, Ipv4Addr),
    storage: S,
    subnet_mask: Ipv4Addr,
    routers: Vec<Ipv4Addr>,
    domain_name_servers: Vec<Ipv4Addr>,
    static_routes: Vec<(Ipv4Addr, Ipv4Addr)>,
    classless_static_routes: Vec<(Ipv4Addr, Ipv4Addr, Ipv4Addr)>,
    #[allow(unused)]
    bpf_num_threads_size: Option<usize>,
}

impl<S> ServerBuilder<S>
where
    S: Storage,
{
    /// Builds a server future.
    ///
    /// * `server_ip_address`
    /// The address clients will receive in the `dhcp_server_id` option.
    /// Is usually set to needed network interface address.
    ///
    /// * `iface_name`
    /// The interface the server should work on. Is required for ARP injection.
    /// Something like `ens33` on Linux or like `Ethernet` on Windows.
    ///
    /// * `static_address_range`
    /// An inclusive IPv4 address range. Gaps may be implemented later.
    ///
    /// * `dynamic_address_range`
    /// An inclusive IPv4 address range. Gaps may be implemented later.
    ///
    /// * `storage`
    /// The `Storage` trait object. The trait must be implemented by a crate user.
    ///
    /// * `subnet_mask`
    /// Static data for client configuration.
    ///
    /// * `routers`
    /// Static data for client configuration.
    ///
    /// * `domain_name_servers`
    /// Static data for client configuration.
    ///
    /// * `static_routes`
    /// Static data for client configuration.
    ///
    /// * `classless_static_routes`
    /// Static data for client configuration.
    ///
    pub fn new(
        server_ip_address: Ipv4Addr,
        iface_name: String,
        static_address_range: (Ipv4Addr, Ipv4Addr),
        dynamic_address_range: (Ipv4Addr, Ipv4Addr),
        storage: S,
        subnet_mask: Ipv4Addr,
        routers: Vec<Ipv4Addr>,
        domain_name_servers: Vec<Ipv4Addr>,
        static_routes: Vec<(Ipv4Addr, Ipv4Addr)>,
        classless_static_routes: Vec<(Ipv4Addr, Ipv4Addr, Ipv4Addr)>,
    ) -> Self {
        ServerBuilder {
            server_ip_address,
            iface_name,
            static_address_range,
            dynamic_address_range,
            storage,
            subnet_mask,
            routers,
            domain_name_servers,
            static_routes,
            classless_static_routes,
            bpf_num_threads_size: None,
        }
    }

    /// Sets the CPU pool size used for BPF communication.
    ///
    /// If not called during building, the BPF object will use its default pool size.
    #[cfg(any(target_os = "freebsd", target_os = "macos"))]
    pub fn with_bpf_num_threads(&mut self, bpf_num_threads_size: usize) -> &mut Self {
        self.bpf_num_threads_size = Some(bpf_num_threads_size);
        self
    }

    /// Consumes the builder and returns the built server.
    pub async fn finish(self) -> io::Result<Server<S>> {
        Server::new(
            self.server_ip_address,
            self.iface_name,
            self.static_address_range,
            self.dynamic_address_range,
            self.storage,
            self.subnet_mask,
            self.routers,
            self.domain_name_servers,
            self.static_routes,
            self.classless_static_routes,
            self.bpf_num_threads_size,
        ).await
    }

    /// Consumes the builder and returns the built server.
    pub fn finish_with_channel<C>(self, channel: C) -> io::Result<GenericServer<S,C>>
    where
        C: Stream<Item = Result<DhcpStreamItem, io::Error>> +
            Sink<DhcpSinkItem, Error = io::Error>,
    {
        GenericServer::with_channel(
            channel,
            self.server_ip_address,
            self.iface_name,
            self.static_address_range,
            self.dynamic_address_range,
            self.storage,
            self.subnet_mask,
            self.routers,
            self.domain_name_servers,
            self.static_routes,
            self.classless_static_routes,
            self.bpf_num_threads_size,
        )
    }

}

pub type Server<S> = GenericServer<S, DhcpFramed<UdpSocket>>;
/// The struct implementing the `Future` trait.
#[pin_project]
pub struct GenericServer<S, C>
where
    S: Storage,
    C: Stream<Item = Result<DhcpStreamItem, io::Error>> +
        Sink<DhcpSinkItem, Error = io::Error>,
{
    /// The server UDP socket.
    #[pin]
    socket: C,
    /// The IP address the server is hosted on.
    server_ip_address: Ipv4Addr,
    /// The interface the server works on.
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    iface_name: String,
    /// The DHCP message building helper.
    builder: MessageBuilder,
    /// The DHCP database using a persistent storage object.
    database: Database<S>,
    /// The asynchronous `netsh` processes used to work with ARP entries.
    #[cfg(target_os = "windows")]
    #[pin]
    arp: Option<dhcp_arp::Arp>,
    /// The object encapsulating BPF functionality.
    #[cfg(any(target_os = "freebsd", target_os = "macos"))]
    bpf_data: BpfData,
}

impl<S> Server<S>
where
    S: Storage,
{
    /// Creates a server future.
    #[allow(unused_variables)]
    async fn new(
        server_ip_address: Ipv4Addr,
        iface_name: String,
        static_address_range: (Ipv4Addr, Ipv4Addr),
        dynamic_address_range: (Ipv4Addr, Ipv4Addr),
        storage: S,
        subnet_mask: Ipv4Addr,
        routers: Vec<Ipv4Addr>,
        domain_name_servers: Vec<Ipv4Addr>,
        static_routes: Vec<(Ipv4Addr, Ipv4Addr)>,
        classless_static_routes: Vec<(Ipv4Addr, Ipv4Addr, Ipv4Addr)>,
        bpf_num_threads_size: Option<usize>,
    ) -> io::Result<Self> {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), DHCP_PORT_SERVER);
        let socket = UdpSocket::bind(&addr).await?;
        socket.set_broadcast(true)?;

        // Older Rust versions hadn't allowed method calls on type aliases
        // so it's `DhcpFramed::<UdpSocket>`, and not a type alias
        let socket = DhcpFramed::<UdpSocket>::new(socket)?;
        let hostname = hostname::get_hostname();

        let builder = MessageBuilder::new(
            server_ip_address,
            hostname,
            subnet_mask,
            routers,
            domain_name_servers,
            static_routes,
            classless_static_routes,
        );

        let database = Database::new(static_address_range, dynamic_address_range, storage);

        Ok(Server {
            socket,
            server_ip_address,
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            iface_name: iface_name.to_owned(),
            builder,
            database,
            #[cfg(target_os = "windows")]
            arp: None,
            #[cfg(any(target_os = "freebsd", target_os = "macos"))]
            bpf_data: BpfData::new(&iface_name, bpf_num_threads_size)?,
        })
    }
}

impl<S, C> GenericServer<S, C>
where
    S: Storage,
    C: Stream<Item = Result<DhcpStreamItem, io::Error>> +
        Sink<DhcpSinkItem, Error = io::Error>,
{
    /// Creates a server future.
    #[allow(unused_variables)]
    pub fn with_channel(
        channel: C,
        server_ip_address: Ipv4Addr,
        iface_name: String,
        static_address_range: (Ipv4Addr, Ipv4Addr),
        dynamic_address_range: (Ipv4Addr, Ipv4Addr),
        storage: S,
        subnet_mask: Ipv4Addr,
        routers: Vec<Ipv4Addr>,
        domain_name_servers: Vec<Ipv4Addr>,
        static_routes: Vec<(Ipv4Addr, Ipv4Addr)>,
        classless_static_routes: Vec<(Ipv4Addr, Ipv4Addr, Ipv4Addr)>,
        bpf_num_threads_size: Option<usize>,
    ) -> io::Result<Self> {
        let hostname = hostname::get_hostname();

        let builder = MessageBuilder::new(
            server_ip_address,
            hostname,
            subnet_mask,
            routers,
            domain_name_servers,
            static_routes,
            classless_static_routes,
        );

        let database = Database::new(static_address_range, dynamic_address_range, storage);

        Ok(GenericServer {
            socket: channel,
            server_ip_address,
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            iface_name: iface_name.to_owned(),
            builder,
            database,
            #[cfg(target_os = "windows")]
            arp: None,
            #[cfg(any(target_os = "freebsd", target_os = "macos"))]
            bpf_data: BpfData::new(&iface_name, bpf_num_threads_size)?,
        })
    }

    /// Chooses the destination IP according to RFC 2131 rules.
    ///
    /// Performs the ARP query in hardware unicast cases and sets the `arp` field
    /// if ARP processing is expected to be too long for the tokio reactor.
    /// The bool flag is `true` if hardware unicast is required.
    fn destination(self: &mut Pin<&mut Self>, request: &Message, response: &Message) -> (Ipv4Addr, bool) {
        if !request.client_ip_address.is_unspecified() {
            return (request.client_ip_address, false);
        }

        if request.is_broadcast {
            return (Ipv4Addr::new(255, 255, 255, 255), false);
        }

        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            info!(
                "Injecting an ARP entry {} -> {}",
                request.client_hardware_address, response.your_ip_address,
            );
            match dhcp_arp::add(
                request.client_hardware_address,
                response.your_ip_address,
                self.as_mut().project().iface_name.to_owned(),
            ) {
                #[cfg(target_os = "windows")]
                Ok(result) => {
                    self.as_mut().project().arp.set(Some(result));
                }
                Err(error) => error!("ARP error: {:?}", error),
                _ => {}
            }
        }

        (response.your_ip_address, true)

        /*
        RFC 2131 §4.1
        If unicasting is not possible, the message
        MAY be sent as an IP broadcast using an IP broadcast address
        (preferably 0xffffffff) as the IP destination address and the link-
        layer broadcast address as the link-layer destination address.

        Note: I don't know yet when unicasting is not possible.
        */
    }

    /// Sends a response using OS-specific features.
    #[allow(unused)]
    fn send_response(
        mut self: &mut Pin<&mut Self>,
        response: Message,
        destination: Ipv4Addr,
        hw_unicast: bool,
        max_size: Option<u16>,
    ) -> io::Result<()> {
        log_send!(response, destination);

        #[cfg(any(target_os = "freebsd", target_os = "macos"))]
        {
            if hw_unicast {
                return self.as_mut().project().bpf_data.send(
                    &self.server_ip_address,
                    &destination,
                    response,
                    max_size,
                );
            }
        }

        let destination = SocketAddr::new(IpAddr::V4(destination), DHCP_PORT_CLIENT);
        match self.as_mut().project().socket.start_send((destination, (response, max_size))) {
            Ok(()) => {},
            Err(error) => {
                warn!("Socket error: {}", error);
                return Err(error);
            },
        };
        Ok(())
    }

    fn database<'a>(self: &'a mut Pin<&mut Self>) -> &'a mut Database<S> {
        self.as_mut().project().database
    }

    fn builder<'a>(self: &'a mut Pin<&mut Self>) -> &'a mut MessageBuilder {
        self.as_mut().project().builder
    }
}

impl<S, C> Future for GenericServer<S, C>
where
    S: Storage,
    C: Stream<Item = Result<DhcpStreamItem, io::Error>> +
        Sink<DhcpSinkItem, Error = io::Error>,
{
    type Output = Result<Infallible, io::Error>;

    /// Works infinite time.
    ///
    /// [RFC 2131](https://tools.ietf.org/html/rfc2131)
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            #[cfg(target_os = "windows")]
            {
                poll_arp!(self, arp);
            }
            poll_flush!(self, socket, cx);
            let (addr, request) = poll_stream!(self, socket, cx);
            log_receive!(request, addr.ip());
            let dhcp_message_type = validate!(request, addr.ip());

            if let Some(dhcp_server_id) = request.options.dhcp_server_id {
                if dhcp_server_id != self.server_ip_address {
                    warn!("Ignoring a message destined for server {}", dhcp_server_id);
                    continue;
                }
            }

            /*
            RFC 2131 §4.1
            If the  'ciaddr' field is nonzero, then the server unicasts
            DHCPOFFER and DHCPACK messages to the address in 'ciaddr'.
            If 'ciaddr' is zero, and the broadcast bit is set, then the server
            broadcasts DHCPOFFER and DHCPACK messages to 0xffffffff. If the
            broadcast bit is not set and the 'ciaddr' is zero, then the server
            unicasts DHCPOFFER and DHCPACK messages to the client's hardware
            address and 'yiaddr' address. In all cases, when 'giaddr' is zero,
            the server broadcasts any DHCPNAK messages to 0xffffffff.
            */

            let client_id = match request.options.client_id {
                Some(ref client_id) => client_id.as_ref(),
                None => request.client_hardware_address.as_bytes(),
            };
            let max_size = request.options.dhcp_max_message_size;

            match dhcp_message_type {
                MessageType::DhcpDiscover => {
                    /*
                    RFC 2131 §4.3.1
                    When a server receives a DHCPDISCOVER message from a client, the
                    server chooses a network address for the requesting client.  If no
                    address is available, the server may choose to report the problem to
                    the system administrator.
                    */

                    match self.database().allocate(
                        client_id,
                        request.options.address_time,
                        request.options.address_request,
                    ) {
                        Ok(offer) => {
                            let response = self.builder.dhcp_discover_to_offer(&request, &offer);
                            let (destination, hw_unicast) = self.destination(&request, &response);
                            self.send_response(response, destination, hw_unicast, max_size)?;
                        }
                        Err(error) => warn!("Address allocation error: {}", error.to_string()),
                    };
                }
                MessageType::DhcpRequest => {
                    /*
                    RFC 2131 §4.3.2
                    A DHCPREQUEST message may come from a client responding to a
                    DHCPOFFER message from a server, from a client verifying a previously
                    allocated IP address or from a client extending the lease on a
                    network address.  If the DHCPREQUEST message contains a 'server
                    identifier' option, the message is in response to a DHCPOFFER
                    message.  Otherwise, the message is a request to verify or extend an
                    existing lease.

                    RFC 2131 §4.3.6 (table 4)
                    ---------------------------------------------------------------------
                    |              |INIT-REBOOT  |SELECTING    |RENEWING     |REBINDING |
                    ---------------------------------------------------------------------
                    |broad/unicast |broadcast    |broadcast    |unicast      |broadcast |
                    |server-ip     |MUST NOT     |MUST         |MUST NOT     |MUST NOT  |
                    |requested-ip  |MUST         |MUST         |MUST NOT     |MUST NOT  |
                    |ciaddr        |zero         |zero         |IP address   |IP address|
                    ---------------------------------------------------------------------

                    Note: server-ip     = request.options.dhcp_server_id
                          ciaddr        = request.client_ip_address
                          requested-ip  = request.options.address_request
                    */

                    // the client is in the SELECTING state
                    if request.options.dhcp_server_id.is_some() {
                        let address = expect!(request.options.address_request);
                        let lease_time = request.options.address_time;

                        match self.database().assign(client_id, &address, lease_time) {
                            Ok(ack) => {
                                let response = self.builder.dhcp_request_to_ack(&request, &ack);
                                let (destination, hw_unicast) =
                                    self.destination(&request, &response);
                                self.send_response(response, destination, hw_unicast, max_size)?;
                            }
                            Err(error) => {
                                warn!("Address assignment error: {}", error.to_string());
                                let response = self.builder.dhcp_request_to_nak(&request, &error);
                                let destination = Ipv4Addr::new(255, 255, 255, 255);
                                self.send_response(response, destination, false, max_size)?;
                            }
                        };
                        continue;
                    }

                    // the client is in the INIT-REBOOT state
                    if request.client_ip_address.is_unspecified() {
                        let address = expect!(request.options.address_request);

                        match self.database.check(client_id, &address) {
                            Ok(ack) => {
                                let response = self.builder.dhcp_request_to_ack(&request, &ack);
                                let (destination, hw_unicast) =
                                    self.destination(&request, &response);
                                self.send_response(response, destination, hw_unicast, max_size)?;
                            }
                            Err(error) => {
                                warn!("Address checking error: {}", error.to_string());
                                if let LeaseInvalid = error {
                                    let response =
                                        self.builder().dhcp_request_to_nak(&request, &error);
                                    let destination = Ipv4Addr::new(255, 255, 255, 255);
                                    self.send_response(response, destination, false, max_size)?;
                                }
                                /*
                                RFC 2131 §4.3.2
                                If the DHCP server has no record of this client, then it MUST
                                remain silent, and MAY output a warning to the network administrator.
                                */
                            }
                        }
                        continue;
                    }

                    // the client is in the RENEWING or REBINDING state
                    let lease_time = request.options.address_time;
                    match self
                        .database()
                        .renew(client_id, &request.client_ip_address, lease_time)
                    {
                        Ok(ack) => {
                            let response = self.builder().dhcp_request_to_ack(&request, &ack);
                            let (destination, hw_unicast) = self.destination(&request, &response);
                            self.send_response(response, destination, hw_unicast, max_size)?;
                        }
                        Err(error) => warn!("Address checking error: {}", error.to_string()),
                    }
                }
                MessageType::DhcpDecline => {
                    /*
                    RFC 2131 §4.3.3
                    If the server receives a DHCPDECLINE message, the client has
                    discovered through some other means that the suggested network
                    address is already in use.  The server MUST mark the network address
                    as not available and SHOULD notify the local system administrator of
                    a possible configuration problem.
                    */

                    let address = expect!(request.options.address_request);
                    match self.database().freeze(&address) {
                        Ok(_) => info!("Address {} has been marked as unavailable", address),
                        Err(error) => warn!("Address freezing error: {}", error.to_string()),
                    };
                }
                MessageType::DhcpRelease => {
                    /*
                    RFC 2131 §4.3.4
                    Upon receipt of a DHCPRELEASE message, the server marks the network
                    address as not allocated.  The server SHOULD retain a record of the
                    client's initialization parameters for possible reuse in response to
                    subsequent requests from the client.
                    */

                    let address = request.client_ip_address;
                    match self.database().deallocate(client_id, &address) {
                        Ok(_) => info!("Address {} has been released", address),
                        Err(error) => warn!("Address releasing error: {}", error.to_string()),
                    };
                }
                MessageType::DhcpInform => {
                    /*
                    RFC 2131 §4.3.5
                    The server responds to a DHCPINFORM message by sending a DHCPACK
                    message directly to the address given in the 'ciaddr' field of the
                    DHCPINFORM message.  The server MUST NOT send a lease expiration time
                    to the client and SHOULD NOT fill in 'yiaddr'.
                    */

                    info!(
                        "Address {} has been taken by some client manually",
                        request.client_ip_address
                    );
                    let response = self.builder.dhcp_inform_to_ack(&request, "Accepted");
                    let (destination, hw_unicast) = self.destination(&request, &response);
                    self.send_response(response, destination, hw_unicast, max_size)?;
                }
                _ => {}
            }
        }
    }
}
