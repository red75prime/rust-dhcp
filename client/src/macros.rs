//! Macro functions used in the `Client:poll` method.

/// A panic indicates a bug in the application logic.
macro_rules! expect (
    ($option:expr) => (
        $option.expect("A bug in the Option setting logic")
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
macro_rules! log_receive(
    ($message:expr, $source:expr) => (
        info!("Received {} from {}", expect!($message.options.dhcp_message_type), $source);
        debug!("{}", $message);
    );
);

/// By design the pending message must be flushed before sending the next one.
macro_rules! start_send (
    ($socket:expr, $address:expr, $message:expr) => (
        $socket.start_send(($address, $message))?
    );
);

/// Just to move some code from the overwhelmed `poll` method.
macro_rules! poll_complete (
    ($socket:expr, $cx:expr) => (
        match Pin::new(&mut $socket).poll_ready($cx) {
            Poll::Ready(Ok(())) => {},
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => {
                warn!("Socket error: {}", error);
                // continue;
                return Poll::Ready(Some(Err(error)));
            },
        }
    );
);

/// Just to move some code from the overwhelmed `poll` method.
macro_rules! validate (
    ($message:expr, $address:expr) => (
        match $message.validate() {
            Ok(dhcp_message_type) => dhcp_message_type,
            Err(error) => {
                warn!("The response from {} is invalid: {} {}", $address, error, $message);
                continue;
            },
        };
    );
);

/// Just to move some code from the overwhelmed `poll` method.
macro_rules! check_xid (
    ($yours:expr, $response:expr) => (
        if $response != $yours {
            warn!("Got a response with wrong transaction ID: {} (yours is {})", $response, $yours);
            continue;
        }
    );
);

/// Just to move some code from the overwhelmed `poll` method.
macro_rules! check_message_type (
    ($message:expr, $needed:pat) => (
        if let $needed = $message {} else {
            warn!("Got an unexpected DHCP message type {}", $message);
            continue;
        }
    );
);

/// Just to move some code from the overwhelmed `poll` method.
macro_rules! poll_delay (
    ($delay:expr, $cx:expr) => (
        if let Some(delay) = $delay.as_pin_mut() {
            match delay.poll($cx) {
                Poll::Ready(()) => {},
                Poll::Pending => return Poll::Pending,
//                Err(error) => panic!("Timer error: {}", error),
            }
        } else {
            panic!("A bug in the timer setting logic");
        }
    );
);

/// Just to move some code from the overwhelmed `poll` method.
macro_rules! poll_backoff (
    ($backoff:expr, $cx:expr) => (
        if let Some(backoff) = $backoff.as_pin_mut() {
            match backoff.poll_next($cx) {
                Poll::Ready(Some((secs, expired))) => {
                    warn!("No responses after {} seconds", secs);
                    if expired {
                        return Poll::Ready(Some(Err(io::Error::new(io::ErrorKind::TimedOut, "Timeout"))));
                    }
                },
                Poll::Ready(None) => panic!("Timer returned None"),
                Poll::Pending => return Poll::Pending,
//                Err(error) => panic!("Timer error: {}", error),
            }
        } else {
            panic!("A bug in the timer setting logic");
        }
    );
    ($backoff:expr, $cx:expr, $revert:expr, $restart:expr) => (
        if let Some(backoff) = $backoff.as_pin_mut() {
            match backoff.poll_next($cx) {
                Poll::Ready(Some((secs, expired))) => {
                    warn!("No responses after {} seconds", secs);
                    if expired {
                        $restart
                    } else {
                        $revert
                    }
                },
                Poll::Ready(None) => panic!("Timer returned None"),
                Poll::Pending => return Poll::Pending,
//                Err(error) => panic!("Timer error: {}", error),
            }
        } else {
            panic!("A bug in the timer setting logic");
        }
    );
);

/// Just to move some code from the overwhelmed `poll` method.
macro_rules! poll_forthon (
    ($forthon:expr, $cx:expr, $revert:expr, $restart:expr) => (
        if let Some(forthon) = $forthon.as_pin_mut() {
            match forthon.poll_next($cx) {
                Poll::Ready(Some((secs, expired))) => {
                    warn!("No responses after {} seconds", secs);
                    if expired {
                        $restart
                    } else {
                        $revert
                    }
                },
                Poll::Ready(None) => panic!("Timer returned None"),
                Poll::Pending => return Poll::Pending,
//                Err(error) => panic!("Timer error: {}", error),
            }
        } else {
            panic!("A bug in the timer setting logic");
        }
    );
);

/// Panic if there is a bug in the state changing logic.
macro_rules! panic_state(
    ($from:expr, $to:expr) => (
        panic!("Invalid state transcension from {} to {}", $from, $to);
    );
);
