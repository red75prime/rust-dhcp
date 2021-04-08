//! Macro functions used in the `Server:poll` method.

/// A panic indicates a bug in the application logic.
macro_rules! expect (
    ($option:expr) => (
        $option.expect("A bug in DHCP message validation")
    );
);

/// Just to move some code from the overwhelmed `poll` method.
macro_rules! log_receive(
    ($message:expr, $source:expr) => (
        info!("Received {} from {}", expect!($message.options.dhcp_message_type), $source);
        debug!("{}", $message);
    );
);

/// Just to move some code from the overwhelmed `poll` method.
macro_rules! log_send(
    ($message:expr, $destination:expr) => (
        info!("Sending {} to {}", expect!($message.options.dhcp_message_type), $destination);
        debug!("{}", $message);
    );
);

/// Just to move some code from the overwhelmed `poll` method.
macro_rules! poll_stream (
    ($self:ident, $stream:ident, $cx:expr) => (
        match $self.as_mut().project().$stream.poll_next($cx) {
            Poll::Ready(Some(Ok(data))) => data,
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Some(Err(error))) => {
                warn!("Socket error: {}", error);
                return Poll::Ready(Err(error));
            },
            Poll::Ready(None) => {
                error!("Socket stream unexpectedly ended");
                return Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Socket stream unexpectedly ended")));
            }
        };
    );
);

/// Just to move some code from the overwhelmed `poll` method.
macro_rules! validate (
    ($message:expr, $address:expr) => (
        match $message.validate() {
            Ok(dhcp_message_type) => dhcp_message_type,
            Err(error) => {
                warn!("The request from {} is invalid: {}", $address, error);
                continue;
            },
        };
    );
);

/// Just to move some code from the overwhelmed `poll` method.
macro_rules! poll_flush (
    ($self:ident, $sink:ident, $cx:expr) => (
        match $self.as_mut().project().$sink.poll_flush($cx) {
            Poll::Ready(Ok(())) => {},
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => {
                warn!("Socket error: {}", error);
                return Poll::Ready(Err(error));
            },
        }
    );
);

/// Just to move some code from the overwhelmed `poll` method.
#[cfg(target_os = "windows")]
macro_rules! poll_arp (
    ($arp:expr) => (
        let mut ready = false;
        if let Some(ref mut arp) = $arp {
            if let Some(ref mut delete) = arp.0 {
                match delete.poll() {
                    Ok(Async::Ready(_)) => {
                        trace!("The netsh delete process finished.");
                    },
                    Ok(Async::NotReady) => return Ok(Async::NotReady),
                    Err(error) => {
                        warn!("netsh delete process future error: {}", error);
                        continue;
                    },
                }
            }
            arp.0 = None;
            if let Some(ref mut add) = arp.1 {
                let output = match add.poll() {
                    Ok(Async::Ready(output)) => {
                        trace!("The netsh add process finished.");
                        ready = true;
                        output
                    },
                    Ok(Async::NotReady) => return Ok(Async::NotReady),
                    Err(error) => {
                        warn!("netsh add process future error: {}", error);
                        continue;
                    },
                };
                if !output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                    if stdout != "The object already exists." {
                        error!("ARP process error: {}", stdout);
                    }
                }
            }
            arp.1 = None;
        }
        if ready {
            $arp = None;
        }
    );
);
