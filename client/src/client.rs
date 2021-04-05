//! The main DHCP client module.

use std::{net::{IpAddr, Ipv4Addr, SocketAddr}, pin::Pin, task::{Context, Poll}};

use eui48::MacAddress;
use futures::{Future, Sink, Stream};
use hostname;
use pin_project::pin_project;
use tokio::io;

use dhcp_protocol::{Message, MessageType, DHCP_PORT_SERVER};
use switchable_socket::{ModeSwitch, SocketMode};

use builder::MessageBuilder;
use state::{DhcpState, State};

/// May be used to request stuff explicitly.
struct RequestOptions {
    /// Explicit network address request.
    address_request: Option<Ipv4Addr>,
    /// Explicit lease time request.
    address_time: Option<u32>,
}

/// The `Client` future result type.
#[derive(Debug, Clone)]
pub struct Configuration {
    pub your_ip_address: Ipv4Addr,
    pub server_ip_address: Ipv4Addr,
    pub subnet_mask: Option<Ipv4Addr>,
    pub routers: Option<Vec<Ipv4Addr>>,
    pub domain_name_servers: Option<Vec<Ipv4Addr>>,
    pub static_routes: Option<Vec<(Ipv4Addr, Ipv4Addr)>>,
    pub classless_static_routes: Option<Vec<(Ipv4Addr, Ipv4Addr, Ipv4Addr)>>,
}

impl Configuration {
    pub fn from_response(mut response: Message) -> Self {
        /*
        RFC 3442
        If the DHCP server returns both a Classless Static Routes option and
        a Router option, the DHCP client MUST ignore the Router option.
        Similarly, if the DHCP server returns both a Classless Static Routes
        option and a Static Routes option, the DHCP client MUST ignore the
        Static Routes option.
        */
        if response.options.classless_static_routes.is_some() {
            response.options.routers = None;
            response.options.static_routes = None;
        }

        Configuration {
            your_ip_address: response.your_ip_address,
            server_ip_address: response.server_ip_address,
            subnet_mask: response.options.subnet_mask,
            routers: response.options.routers,
            domain_name_servers: response.options.domain_name_servers,
            static_routes: response.options.static_routes,
            classless_static_routes: response.options.classless_static_routes,
        }
    }
}

/// The commands used for `Sink` to send `DHCPRELEASE`, `DHCPDECLINE` and `DHCPINFORM` messages.
#[derive(Clone)]
pub enum Command {
    Release {
        message: Option<String>,
    },
    Decline {
        address: Ipv4Addr,
        message: Option<String>,
    },
    Inform {
        address: Ipv4Addr,
    },
}

type DhcpStreamItem = (SocketAddr, Message);
type DhcpSinkItem = (SocketAddr, (Message, Option<u16>));

/// The struct implementing the `Future` trait.
#[pin_project]
pub struct Client<S> {
    #[pin]
    io: S,
    builder: MessageBuilder,
    #[pin]
    state: State,
    options: RequestOptions,
}

impl<IO> Client<IO>
where
    IO: Stream<Item = Result<DhcpStreamItem, io::Error>>
        + Sink<DhcpSinkItem, Error = io::Error>
        + ModeSwitch
        + Send
        + Sync,
{
    /// Creates a client future.
    ///
    /// * `stream`
    /// The external socket `Stream` part.
    ///
    /// * `sink`
    /// The external socket `Sink` part.
    ///
    /// * `client_hardware_address`
    /// The mandatory client MAC address.
    ///
    /// * `client_id`
    /// The optional client identifier.
    /// If `None`, is defaulted to the 6-byte MAC address.
    ///
    /// * `hostname`
    /// May be explicitly set by a client user.
    /// Otherwise it is defaulted to the machine hostname.
    /// If the hostname cannot be get, remains unset.
    ///
    /// * `server_address`
    /// The DHCP server address.
    /// Set it if your know the server address.
    /// If set, the client communicates with the server using unicast.
    /// Otherwise, broadcasting to 255.255.255.255 is used.
    ///
    /// * `client_address`
    /// The previous client address.
    /// Set it if you want to reacquire your previous network address.
    /// If set, the client is started in INIT-REBOOT state.
    /// If unset, the client is started in INIT state.
    ///
    /// * `address_request`
    /// The requested network address.
    /// Set it if you want to request a specific network address.
    /// If not set, a server will give you either
    /// your current or previous address, or an address from its dynamic pool.
    ///
    /// * `address_time`
    /// The requested lease time.
    /// If not set, a server will choose the lease time by itthis.
    /// The server may lease the address for different amount of time if it decides so.
    ///
    /// * `max_message_size`
    /// The maximum DHCP message size.
    ///
    /// * `request_static_routes`
    /// Client requests static routes and static classless routes.
    /// Maximum DHCP message size should be increased to accomodate them.
    pub fn new(
        io: IO,
        client_hardware_address: MacAddress,
        client_id: Option<Vec<u8>>,
        hostname: Option<String>,
        server_address: Option<Ipv4Addr>,
        client_address: Option<Ipv4Addr>,
        address_request: Option<Ipv4Addr>,
        address_time: Option<u32>,
        max_message_size: Option<u16>,
        request_static_routes: bool,
    ) -> Self {
        let hostname: Option<String> = if let Some(hostname) = hostname {
            Some(hostname)
        } else {
            hostname::get_hostname()
        };

        let client_id = if let Some(client_id) = client_id {
            client_id
        } else {
            // https://tools.ietf.org/html/rfc2132#section-9.14
            let mut client_id = Vec::with_capacity(client_hardware_address.as_bytes().len()+1);
            client_id.push(dhcp_protocol::HardwareType::Ethernet as u8);
            client_id.extend_from_slice(client_hardware_address.as_bytes());
            client_id
        };

        let builder = MessageBuilder::new(
            client_hardware_address,
            client_id,
            hostname,
            max_message_size,
            request_static_routes,
        );

        let mut options = RequestOptions {
            address_request,
            address_time,
        };

        let dhcp_state = match client_address {
            Some(ip) => {
                options.address_request = Some(ip);
                DhcpState::InitReboot
            }
            None => DhcpState::Init,
        };

        let state = State::new(dhcp_state, server_address, false);

        Client {
            io,
            builder,
            state,
            options,
        }
    }

    /// Chooses the packet destination address according to the RFC 2131 rules.
    fn destination(self: &mut Pin<&mut Self>) -> Ipv4Addr {
        /*
        RFC 2131 §4.4.4
        The DHCP client broadcasts DhcpDiscover, DhcpRequest and DHCPINFORM
        messages, unless the client knows the address of a DHCP server. The
        client unicasts DHCPRELEASE messages to the server. Because the
        client is declining the use of the IP address supplied by the server,
        the client broadcasts DHCPDECLINE messages.

        When the DHCP client knows the address of a DHCP server, in either
        INIT or REBOOTING state, the client may use that address in the
        DhcpDiscover or DhcpRequest rather than the IP broadcast address.
        The client may also use unicast to send DHCPINFORM messages to a
        known DHCP server.  If the client receives no response to DHCP
        messages sent to the IP address of a known DHCP server, the DHCP
        client reverts to using the IP broadcast address.
        */

        let mut this = self.as_mut().project();
        // Raw socket may not support unicasting
        if this.io.mode() == SocketMode::Raw {
            Ipv4Addr::new(255, 255, 255, 255)
        } else {
            if let Some(dhcp_server_id) = this.state.dhcp_server_id() {
                dhcp_server_id
            } else {
                Ipv4Addr::new(255, 255, 255, 255)
            }
        }
    }

    /// Sends a request.
    fn send_request(self: &mut Pin<&mut Self>, request: Message) -> io::Result<()> {
        let destination = self.destination();
        let this = self.as_mut().project();
        log_send!(request, destination);

        let destination = SocketAddr::new(IpAddr::V4(destination), DHCP_PORT_SERVER);
        start_send!(this.io, destination, (request, None));
        Ok(())
    }
}

impl<IO> Stream for Client<IO>
where
    IO: Stream<Item = Result<DhcpStreamItem, io::Error>>
        + Sink<DhcpSinkItem, Error = io::Error>
        + ModeSwitch
        + Send
        + Sync,
{
    type Item = Result<Configuration, io::Error>;

    /// Yields a `Configuration` after each configuration update.
    ///
    /// The DHCP client lifecycle (RFC 2131)
    ///  --------                               -------
    /// |        | +-------------------------->|       |<-------------------+
    /// | INIT-  | |     +-------------------->| INIT  |                    |
    /// | REBOOT |DhcpNak/         +---------->|       |<---+               |
    /// |        |Restart|         |            -------     |               |
    ///  --------  |  DhcpNak/     |               |        |               |
    ///     |      Discard offer   |      -/Send DhcpDiscover               |
    /// -/Send DhcpRequest         |               |        |               |
    ///     |      |     |      DhcpAck            v        |               |
    ///  -----------     |   (not accept.)/   -----------   |               |
    /// |           |    |  Send DHCPDECLINE |           |  |               |
    /// | REBOOTING |    |         |         | SELECTING |<----+            |
    /// |           |    |        /          |           |  |  |DHCPOFFER/  |
    ///  -----------     |       /            -----------   |  |Collect     |
    ///     |            |      /                  |   |    |  |  replies   |
    /// DhcpAck/         |     /  +----------------+   +-------+            |
    /// Record lease, set|    |   v   Select offer/         |               |
    /// timers T1, T2   ------------  send DhcpRequest      |               |
    ///     |   +----->|            |             DhcpNak, Lease expired/   |
    ///     |   |      | REQUESTING |                  Halt network         |
    ///     DHCPOFFER/ |            |                       |               |
    ///     Discard     ------------                        |               |
    ///     |   |        |        |                   -----------           |
    ///     |   +--------+     DhcpAck/              |           |          |
    ///     |              Record lease, set    -----| REBINDING |          |
    ///     |                timers T1, T2     /     |           |          |
    ///     |                     |        DhcpAck/   -----------           |
    ///     |                     v     Record lease, set   ^               |
    ///     +----------------> -------      /timers T1,T2   |               |
    ///                +----->|       |<---+                |               |
    ///                |      | BOUND |<---+                |               |
    ///   DHCPOFFER, DhcpAck, |       |    |            T2 expires/   DhcpNak/
    ///    DhcpNak/Discard     -------     |             Broadcast  Halt network
    ///                |       | |         |            DhcpRequest         |
    ///                +-------+ |        DhcpAck/          |               |
    ///                     T1 expires/   Record lease, set |               |
    ///                  Send DhcpRequest timers T1, T2     |               |
    ///                  to leasing server |                |               |
    ///                          |   ----------             |               |
    ///                          |  |          |------------+               |
    ///                          +->| RENEWING |                            |
    ///                             |          |----------------------------+
    ///                              ----------
    ///
    /// [RFC 2131](https://tools.ietf.org/html/rfc2131)
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            poll_complete!(self.as_mut().project().io, cx);
            match self.as_mut().project().state.dhcp_state() {
                current @ DhcpState::Init => {
                    /*
                    RFC 2131 §4.4.1
                    The client begins in INIT state and forms a DhcpDiscover message.
                    The client MAY suggest a network address and/or lease time by including
                    the 'requested IP address' and 'IP address lease time' options.
                    */
                    self.as_mut().project().io.switch_to(SocketMode::Raw)?;
                    self.as_mut().project().state.transcend(current, DhcpState::Selecting, None);
                }
                current @ DhcpState::Selecting => {
                    /*
                    RFC 2131 §4.4.1
                    If the parameters are acceptable, the client records the address of
                    the server that supplied the parameters from the 'server identifier'
                    field and sends that address in the 'server identifier' field of a
                    DhcpRequest broadcast message.
                    */
                    let xid = self.as_mut().project().state.xid();
                    let is_broadcast = self.as_mut().project().state.is_broadcast();
                    let address_request = self.as_mut().project().options.address_request;
                    let address_time = self.as_mut().project().options.address_time;
                    let request = self.as_mut().project().builder.discover(
                        xid,
                        is_broadcast,
                        address_request,
                        address_time,
                    );

                    self.send_request(request)?;
                    self.as_mut().project().state
                        .transcend(current, DhcpState::SelectingSent, None);
                }
                current @ DhcpState::SelectingSent => {
                    let (addr, response) = match self.as_mut().project().io.poll_next(cx) {
                        Poll::Ready(Some(Ok(data))) => data,
                        Poll::Ready(None) => {
                            error!("UDP socket reports end of stream");
                            continue;
                        }
                        Poll::Pending => {
                            poll_backoff!(self.as_mut().project().state.project().timer_offer, cx);
                            self.as_mut().project().state.transcend(current, DhcpState::Selecting, None);
                            continue;
                        }
                        Poll::Ready(Some(Err(error))) => {
                            warn!("Socket error: {}", error);
                            continue;
                        }
                    };

                    let dhcp_message_type = validate!(response, addr);
                    log_receive!(response, addr.ip());
                    check_xid!(self.as_mut().project().state.xid(), response.transaction_id);
                    check_message_type!(dhcp_message_type, MessageType::DhcpOffer);
                    self.as_mut().project().state
                        .transcend(current, DhcpState::Requesting, Some(&response));
                }
                current @ DhcpState::Requesting => {
                    /*
                    RFC 2131 §4.4.1
                    Once the DhcpAck message from the server arrives,
                    the client is initialized and moves to BOUND state.
                    */

                    let xid = self.as_mut().project().state.xid();
                    let is_broadcast = self.as_mut().project().state.is_broadcast();
                    let offered_address = self.as_mut().project().state.offered_address();
                    let offered_time = Some(self.as_mut().project().state.offered_time());
                    let dhcp_server_id = expect!(self.as_mut().project().state.dhcp_server_id());
                    let request = self.as_mut().project().builder.request_selecting(
                        xid,
                        is_broadcast,
                        offered_address,
                        offered_time,
                        dhcp_server_id,
                    );

                    self.send_request(request)?;
                    self.as_mut().project().state
                        .transcend(current, DhcpState::RequestingSent, None);
                }
                current @ DhcpState::RequestingSent => {
                    let (addr, response) = match self.as_mut().project().io.poll_next(cx) {
                        Poll::Ready(Some(Ok(data))) => data,
                        Poll::Ready(None) => {
                            error!("UDP socket reports end of stream");
                            continue;
                        }
                        Poll::Pending => {
                            let next = poll_backoff!(
                                self.as_mut().project().state.project().timer_ack,
                                cx,
                                DhcpState::Requesting,
                                DhcpState::Init
                            );
                            self.as_mut().project().state.transcend(current, next, None);
                            continue;
                        }
                        Poll::Ready(Some(Err(error))) => {
                            warn!("Socket error: {}", error);
                            continue;
                        }
                    };

                    let dhcp_message_type = validate!(response, addr);
                    log_receive!(response, addr.ip());
                    check_xid!(self.as_mut().project().state.xid(), response.transaction_id);

                    match dhcp_message_type {
                        MessageType::DhcpNak => {
                            warn!("Got {} in {} state", dhcp_message_type, current);
                            self.as_mut().project().state.transcend(current, DhcpState::Init, None);
                            continue;
                        }
                        MessageType::DhcpAck => {}
                        _ => {
                            warn!("Got an unexpected DHCP message type {}", dhcp_message_type);
                            continue;
                        }
                    }

                    self.as_mut().project().state
                        .transcend(current, DhcpState::Bound, Some(&response));
                    return Poll::Ready(Some(Ok(Configuration::from_response(response))));
                }

                current @ DhcpState::InitReboot => {
                    /*
                    RFC 2131 §4.4.2
                    The client begins in INIT-REBOOT state and sends a DhcpRequest
                    message.  The client MUST insert its known network address as a
                    'requested IP address' option in the DhcpRequest message.
                    */
                    self.as_mut().project().io.switch_to(SocketMode::Raw)?;
                    self.as_mut().project().state.transcend(current, DhcpState::Rebooting, None);
                }
                current @ DhcpState::Rebooting => {
                    /*
                    RFC 2131 §4.4.2
                    Once a DhcpAck message with an 'xid' field matching that in the
                    client's DhcpRequest message arrives from any server, the client is
                    initialized and moves to BOUND state.
                    */

                    let xid = self.as_mut().project().state.xid();
                    let is_broadcast = self.as_mut().project().state.is_broadcast();
                    let address_request = expect!(self.as_mut().project().options.address_request);
                    let address_time = self.as_mut().project().options.address_time;
                    let request = self.as_mut().project().builder.request_init_reboot(
                        xid,
                        is_broadcast,
                        address_request,
                        address_time,
                    );

                    self.send_request(request)?;
                    self.as_mut().project().state
                        .transcend(current, DhcpState::RebootingSent, None);
                }
                current @ DhcpState::RebootingSent => {
                    let (addr, response) = match self.as_mut().project().io.poll_next(cx) {
                        Poll::Ready(Some(Ok(data))) => data,
                        Poll::Ready(None) => {
                            error!("UDP socket reports end of stream");
                            continue;
                        }
                        Poll::Pending => {
                            let next = poll_backoff!(
                                self.as_mut().project().state.project().timer_ack,
                                cx,
                                DhcpState::Rebooting,
                                DhcpState::Init
                            );
                            self.as_mut().project().state.transcend(current, next, None);
                            continue;
                        }
                        Poll::Ready(Some(Err(error))) => {
                            warn!("Socket error: {}", error);
                            continue;
                        }
                    };

                    let dhcp_message_type = validate!(response, addr);
                    log_receive!(response, addr.ip());
                    check_xid!(self.as_mut().project().state.xid(), response.transaction_id);

                    match dhcp_message_type {
                        MessageType::DhcpNak => {
                            warn!("Got {} in {} state", dhcp_message_type, current);
                            self.as_mut().project().state.transcend(current, DhcpState::Init, None);
                            continue;
                        }
                        MessageType::DhcpAck => {}
                        _ => {
                            warn!("Got an unexpected DHCP message type {}", dhcp_message_type);
                            continue;
                        }
                    }

                    self.as_mut().project().state
                        .transcend(current, DhcpState::Bound, Some(&response));
                    return Poll::Ready(Some(Ok(Configuration::from_response(response))));
                }

                current @ DhcpState::Bound => {
                    /*
                    RFC 2131 §4.4.5
                    At time T1 the client moves to RENEWING state and sends (via unicast)
                    a DHCPREQUEST message to the server to extend its lease.  The client
                    sets the 'ciaddr' field in the DHCPREQUEST to its current network
                    address. The client records the local time at which the DHCPREQUEST
                    message is sent for computation of the lease expiration time.  The
                    client MUST NOT include a 'server identifier' in the DHCPREQUEST
                    message.
                    */
                    poll_delay!(self.as_mut().project().state.project().timer_renewal, cx);
                    self.as_mut().project().io.switch_to(SocketMode::Udp)?;
                    self.as_mut().project().state.transcend(current, DhcpState::Renewing, None);
                }
                current @ DhcpState::Renewing => {
                    /*
                    RFC 2131 §4.4.5
                    If no DHCPACK arrives before time T2, the client moves to REBINDING
                    state and sends (via broadcast) a DHCPREQUEST message to extend its
                    lease.  The client sets the 'ciaddr' field in the DHCPREQUEST to its
                    current network address.  The client MUST NOT include a 'server
                    identifier' in the DHCPREQUEST message.
                    */

                    let xid = self.as_mut().project().state.xid();
                    let is_broadcast = self.as_mut().project().state.is_broadcast();
                    let assigned_address = self.as_mut().project().state.assigned_address();
                    let address_time = self.as_mut().project().options.address_time;
                    let request = self.as_mut().project().builder.request_renew(
                        xid,
                        is_broadcast,
                        assigned_address,
                        address_time,
                    );

                    self.send_request(request)?;
                    self.as_mut().project().state.transcend(current, DhcpState::RenewingSent, None);
                }
                current @ DhcpState::RenewingSent => {
                    let (addr, response) = match self.as_mut().project().io.poll_next(cx) {
                        Poll::Ready(Some(Ok(data))) => data,
                        Poll::Ready(None) => {
                            error!("UDP socket reports end of stream");
                            continue;
                        }
                        Poll::Pending => {
                            let next = poll_forthon!(
                                self.as_mut().project().state.project().timer_rebinding,
                                cx,
                                DhcpState::Renewing,
                                DhcpState::Rebinding
                            );
                            self.as_mut().project().state.transcend(current, next, None);
                            continue;
                        }
                        Poll::Ready(Some(Err(error))) => {
                            warn!("Socket error: {}", error);
                            continue;
                        }
                    };

                    let dhcp_message_type = validate!(response, addr);
                    log_receive!(response, addr.ip());
                    check_xid!(self.as_mut().project().state.xid(), response.transaction_id);
                    check_message_type!(dhcp_message_type, MessageType::DhcpAck);

                    self.as_mut().project().state
                        .transcend(current, DhcpState::Bound, Some(&response));
                    return Poll::Ready(Some(Ok(Configuration::from_response(response))));
                }
                current @ DhcpState::Rebinding => {
                    /*
                    RFC 2131 §4.4.5
                    If the lease expires before the client receives a DHCPACK, the client
                    moves to INIT state, MUST immediately stop any other network
                    processing and requests network initialization parameters as if the
                    client were uninitialized.  If the client then receives a DHCPACK
                    allocating that client its previous network address, the client
                    SHOULD continue network processing.  If the client is given a new
                    network address, it MUST NOT continue using the previous network
                    address and SHOULD notify the local users of the problem.
                    */

                    let xid = self.as_mut().project().state.xid();
                    let is_broadcast = self.as_mut().project().state.is_broadcast();
                    let assigned_address = self.as_mut().project().state.assigned_address();
                    let address_time = self.as_mut().project().options.address_time;
                    let request = self.as_mut().project().builder.request_renew(
                        xid,
                        is_broadcast,
                        assigned_address,
                        address_time,
                    );

                    // RFC 2131 §4.4.5 If no DHCPACK arrives before time T2, the client moves to REBINDING
                    // state and sends (via broadcast) a DHCPREQUEST message to extend its
                    // lease.
                    self.as_mut().project().io.switch_to(SocketMode::Raw)?;

                    self.send_request(request)?;
                    self.as_mut().project().state
                        .transcend(current, DhcpState::RebindingSent, None);
                }
                current @ DhcpState::RebindingSent => {
                    let (addr, response) = match self.as_mut().project().io.poll_next(cx) {
                        Poll::Ready(Some(Ok(data))) => data,
                        Poll::Ready(None) => {
                            error!("UDP socket reports end of stream");
                            continue;
                        }
                        Poll::Pending => {
                            let next = poll_forthon!(
                                self.as_mut().project().state.project().timer_expiration,
                                cx,
                                DhcpState::Rebinding,
                                DhcpState::Init
                            );
                            self.as_mut().project().state.transcend(current, next, None);
                            continue;
                        }
                        Poll::Ready(Some(Err(error))) => {
                            warn!("Socket error: {}", error);
                            continue;
                        }
                    };

                    let dhcp_message_type = validate!(response, addr);
                    log_receive!(response, addr.ip());
                    check_xid!(self.as_mut().project().state.xid(), response.transaction_id);
                    check_message_type!(dhcp_message_type, MessageType::DhcpAck);

                    self.as_mut().project().state
                        .transcend(current, DhcpState::Bound, Some(&response));
                    return Poll::Ready(Some(Ok(Configuration::from_response(response))));
                }
            }
        }
    }
}

impl<IO> Sink<Command> for Client<IO>
where
    IO: Stream<Item = Result<DhcpStreamItem, io::Error>>
        + Sink<DhcpSinkItem, Error = io::Error>
        + Send
        + Sync,
{
    type Error = io::Error;

    /// Translates a `Command` into a DHCP message and sends it to the user provided `Sink`.
    fn start_send(
        mut self: Pin<&mut Self>,
        command: Command,
    ) -> Result<(), Self::Error> {
        let (request, destination) = match command {
            Command::Release { ref message } => {
                let dhcp_server_id = match self.as_mut().project().state.dhcp_server_id() {
                    Some(dhcp_server_id) => dhcp_server_id,
                    None => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrNotAvailable,
                            "Nothing to release",
                        ))
                    }
                };
                let destination = SocketAddr::new(IpAddr::V4(dhcp_server_id), DHCP_PORT_SERVER);
                let xid = self.as_mut().project().state.xid();
                let assigned_address = self.as_mut().project().state.assigned_address();
                let request = self.as_mut().project().builder.release(
                    xid,
                    assigned_address,
                    dhcp_server_id,
                    message.to_owned(),
                );
                (request, destination)
            }
            Command::Decline {
                ref address,
                ref message,
            } => {
                let dhcp_server_id = match self.as_mut().project().state.dhcp_server_id() {
                    Some(dhcp_server_id) => dhcp_server_id,
                    None => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrNotAvailable,
                            "Nothing to decline",
                        ))
                    }
                };
                let destination = SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)),
                    DHCP_PORT_SERVER,
                );
                let xid = self.as_mut().project().state.xid();
                let request = self.as_mut().project().builder.decline(
                    xid,
                    address.to_owned(),
                    dhcp_server_id,
                    message.to_owned(),
                );
                (request, destination)
            }
            Command::Inform { ref address } => {
                let dhcp_server_id = match self.as_mut().project().state.dhcp_server_id() {
                    Some(dhcp_server_id) => dhcp_server_id,
                    None => Ipv4Addr::new(255, 255, 255, 255),
                };
                let destination = SocketAddr::new(IpAddr::V4(dhcp_server_id), DHCP_PORT_SERVER);
                let xid = self.as_mut().project().state.xid();
                let is_broadcast = self.as_mut().project().state.is_broadcast();
                let request = self.as_mut().project().builder.inform(
                    xid,
                    is_broadcast,
                    address.to_owned(),
                );
                (request, destination)
            }
        };

        log_send!(request, destination);
        match self.as_mut().project().io.start_send((destination, (request, None))) {
            Ok(()) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Just a proxy.
    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().io.poll_ready(cx)
    }

    /// Just a proxy.
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().io.poll_flush(cx)
    }

    /// Just a proxy.
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().io.poll_close(cx)
    }
}
