//! The Binary Exponential Forthon™ module.
//!
//! In both RENEWING and REBINDING states, if the client receives no
//! response to its DHCPREQUEST message, the client SHOULD wait one-half
//! of the remaining time until T2 (in RENEWING state) and one-half of
//! the remaining lease time (in REBINDING state), down to a minimum of
//! 60 seconds, before retransmitting the DHCPREQUEST message.

use std::time::Duration;

use std::task::{Context, Poll};
use futures::{Future, Stream};
use tokio::time::{sleep as tokio_sleep, Sleep};
use pin_project::pin_project;

/// Binary exponential Forthon™ algorithm implemented as a `Stream`.
///
/// Yields and eats a half of `left` after each timeout.
#[pin_project]
pub struct Forthon {
    /// The timer itself.
    #[pin]
    timeout: Sleep,
    /// State
    state: ForthonState,
}

struct ForthonState {
    /// Left until deadline.
    left: Duration,
    /// Last sleep duration.
    sleep: Duration,
    /// The timeout is defaulted to it if `left` is less than `minimal`.
    minimal: Duration,
    /// The expiration flag.
    expired: bool,
}

impl ForthonState {
    pub fn new(deadline: Duration, minimal: Duration) -> ForthonState {
        let (sleep, expired) = if deadline < minimal * 2 {
            (deadline, true)
        } else {
            (deadline / 2, false)
        };

        ForthonState {
            left: deadline - sleep,
            sleep,
            minimal,
            expired,
        }
    }

    fn next(&mut self) -> Duration {
        self.sleep = if self.left < self.minimal * 2 {
            self.expired = true;
            self.left
        } else {
            self.left / 2
        };
        self.left -= self.sleep;
        self.sleep
    }
}

impl Forthon {
    /// Constructs a timer and starts it.
    ///
    /// * `deadline`
    /// The duration until expiration.
    ///
    /// * `minimal`
    /// The duration to be slept if `left` is less than it. The last timeout before expiration.
    pub fn new(deadline: Duration, minimal: Duration) -> Forthon {
        let state = ForthonState::new(deadline, minimal);

        Forthon {
            timeout: tokio_sleep(state.sleep),
            state,
        }
    }
}

impl Stream for Forthon {
    type Item = (u64, bool);

    /// Yields seconds slept and the expiration flag.
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        ready!(this.timeout.as_mut().poll(cx));
        let seconds = this.state.sleep.as_secs();
        if this.state.expired {
            return Poll::Ready(Some((seconds, true)));
        }
        this.timeout.set(tokio_sleep(this.state.next()));
        Poll::Ready(Some((seconds, false)))
    }
}
