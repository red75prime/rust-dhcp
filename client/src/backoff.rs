//! The Binary Exponential Backoff module.
//!
//! DHCP clients are responsible for all message retransmission.  The
//! client MUST adopt a retransmission strategy that incorporates a
//! randomized exponential backoff algorithm to determine the delay
//! between retransmissions.  The delay between retransmissions SHOULD be
//! chosen to allow sufficient time for replies from the server to be
//! delivered based on the characteristics of the internetwork between
//! the client and the server.  For example, in a 10Mb/sec Ethernet
//! internetwork, the delay before the first retransmission SHOULD be 4
//! seconds randomized by the value of a uniform random number chosen
//! from the range -1 to +1.  Clients with clocks that provide resolution
//! granularity of less than one second may choose a non-integer
//! randomization value.  The delay before the next retransmission SHOULD
//! be 8 seconds randomized by the value of a uniform number chosen from
//! the range -1 to +1.  The retransmission delay SHOULD be doubled with
//! subsequent retransmissions up to a maximum of 64 seconds.  The client
//! MAY provide an indication of retransmission attempts to the user as
//! an indication of the progress of the configuration process.

use std::time::Duration;

use std::task::{Context, Poll};
use futures::{ready, Future, Stream};
use pin_project::pin_project;
use rand::{self, Rng};
use tokio::time::{sleep as tokio_sleep, Sleep};

/// This `value`, this `-value` or `0` is added to each timeout in seconds.
const AMPLITUDE: i32 = 1;

/// Binary exponential backoff algorithm implemented as a `Stream`.
///
/// Yields after each timeout.
#[pin_project]
pub struct Backoff {
    /// The current timeout without randomization.
    current: Duration,
    /// The current timeout with randomization.
    with_rand: Duration,
    /// The timeout after which the timer is expired.
    maximal: Duration,
    /// The timer itself.
    #[pin]
    timeout: Sleep,
}

impl Backoff {
    /// Constructs a timer and starts it.
    ///
    /// * `minimal`
    /// The initial timeout duration.
    ///
    /// * `maximal`
    /// The maximal timeout duration, inclusively.
    pub fn new(minimal: Duration, maximal: Duration) -> Backoff {
        let with_rand = Self::randomize(&minimal);
        trace!("Backoff::new: sleep {:?}", with_rand);
        Backoff {
            current: minimal,
            with_rand,
            maximal,
            timeout: tokio_sleep(with_rand),
        }
    }

    /// Construct a duration with -1/0/+1 second random offset.
    fn randomize(duration: &Duration) -> Duration {
        let offset: i32 = rand::thread_rng().gen_range(-AMPLITUDE, AMPLITUDE + 1);
        let mut duration = Duration::from(duration.to_owned());
        if offset > 0 {
            duration += Duration::from_secs(offset as u64);
        }
        if offset < 0 {
            duration -= Duration::from_secs((-offset) as u64);
        }
        duration
    }
}

impl Stream for Backoff {
    type Item = (u64, bool);

    /// Yields seconds slept and the expiration flag.
    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        trace!("Backoff::poll_next");
        let mut this = self.project();
        ready!(this.timeout.as_mut().poll(cx));
        let seconds = this.with_rand.as_secs();
        *this.current *= 2;
        *this.with_rand = Self::randomize(this.current);
        this.timeout.set(tokio_sleep(*this.with_rand));
        Poll::Ready(Some((seconds, this.current > this.maximal)))
    }
}
