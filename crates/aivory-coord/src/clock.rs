//! Time sources and the hybrid logical clock.
//!
//! All times are Unix milliseconds. [`Hlc`] orders events of one daemon, and later of many
//! daemons (design `docs/design/04-coordination-sync.md`, "Offline and failure").

use std::cell::Cell;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A source of the current time.
pub trait Clock: Send + Sync + fmt::Debug {
    /// Unix time in milliseconds.
    fn now_ms(&self) -> i64;
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
    }
}

/// A clock that the caller moves by hand. Clones share the same time. Use it in tests.
#[derive(Debug, Clone, Default)]
pub struct ManualClock(Arc<AtomicI64>);

impl ManualClock {
    /// A clock that starts at `ms`.
    pub fn new(ms: i64) -> Self {
        Self(Arc::new(AtomicI64::new(ms)))
    }

    /// Move the clock forward by `ms`.
    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }

    /// Set the clock to `ms`.
    pub fn set(&self, ms: i64) {
        self.0.store(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// A hybrid logical clock value: 48 bits of wall time in milliseconds and 16 bits of counter.
/// Values compare as integers, so the order is stable under clock skew.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Hlc(pub u64);

impl Hlc {
    /// The wall time part in milliseconds.
    pub fn wall_ms(self) -> u64 {
        self.0 >> 16
    }

    /// The counter part.
    pub fn counter(self) -> u16 {
        (self.0 & 0xFFFF) as u16
    }
}

impl fmt::Display for Hlc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.wall_ms(), self.counter())
    }
}

/// The generator of [`Hlc`] values for one daemon.
#[derive(Debug, Default)]
pub struct HlcClock {
    last: Cell<u64>,
}

impl HlcClock {
    /// A generator that continues after `last` (the highest value that the database holds).
    pub fn starting_after(last: Hlc) -> Self {
        Self {
            last: Cell::new(last.0),
        }
    }

    /// The next value for an event at wall time `now_ms`. Each call returns a larger value.
    pub fn tick(&self, now_ms: i64) -> Hlc {
        let wall = u64::try_from(now_ms).unwrap_or(0).min((1 << 47) - 1);
        let last = self.last.get();
        let next = if wall > last >> 16 {
            wall << 16
        } else {
            last.saturating_add(1)
        };
        self.last.set(next);
        Hlc(next)
    }

    /// Merge a value from another daemon (team scope). The next [`tick`](Self::tick) is larger
    /// than `remote` and than every earlier value.
    pub fn observe(&self, remote: Hlc) {
        if remote.0 > self.last.get() {
            self.last.set(remote.0);
        }
    }
}
