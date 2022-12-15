#[cfg(feature = "async")]
use std::time::Duration;
use std::{
    sync::atomic::{AtomicU8, Ordering},
    time::Instant,
};

use crate::backoff;

/// The state keeps the state of signals in both sync and async to make eventing for senders and receivers possible
pub struct State {
    v: AtomicU8,
}

impl Default for State {
    fn default() -> Self {
        Self { v: 0.into() }
    }
}

pub const UNLOCKED: u8 = 0;
pub const TERMINATED: u8 = 1;
pub const LOCKED: u8 = 2;
pub const LOCKED_STARVATION: u8 = 3;
impl State {
    /// Returns the current value of State with recommended Ordering
    #[inline(always)]
    pub fn value(&self) -> u8 {
        self.v.load(Ordering::SeqCst)
    }

    /// Stores value with recommended Ordering in State
    #[inline(always)]
    pub fn store(&self, v: u8) {
        self.v.store(v, Ordering::SeqCst)
    }

    /// Waits synchronously without putting the thread to sleep until the instant time is reached
    /// this function may return with latency after instant time because of spin loop implementation
    #[inline(always)]
    #[must_use = "ignoring wait functions return value will lead to UB"]
    pub fn wait_unlock_until(&self, until: Instant) -> u8 {
        for _ in 0..(1 << 8) {
            let v = self.v.load(Ordering::Relaxed);
            if v < LOCKED {
                return v;
            }
            backoff::spin_hint();
        }
        while Instant::now() < until {
            let v = self.v.load(Ordering::Relaxed);
            if v < LOCKED {
                return v;
            }
            backoff::yield_now();
        }
        self.v.load(Ordering::SeqCst)
    }

    /// Waits synchronously for the signal in sync mode, it should not be used anywhere except a drop of async future
    #[cfg(feature = "async")]
    #[must_use = "ignoring wait functions return value will lead to UB"]
    pub fn wait_indefinitely(&self) -> u8 {
        for _ in 0..(1 << 8) {
            let v = self.v.load(Ordering::Relaxed);
            if v < LOCKED {
                return v;
            }
            backoff::spin_hint();
        }
        let mut sleep_time: u64 = 1 << 10;
        loop {
            backoff::sleep(Duration::from_nanos(sleep_time));
            let v = self.v.load(Ordering::Relaxed);
            if v < LOCKED {
                return v;
            }
            // increase sleep_time gradually to 262 microseconds
            if sleep_time < (1 << 18) {
                sleep_time <<= 1;
            }
        }
    }

    /// Unlocks the state and changes it to a successful state
    #[inline(always)]
    pub unsafe fn unlock(&self) -> bool {
        self.v
            .compare_exchange(LOCKED, UNLOCKED, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
    }

    /// Force unlocks the state without checking for the current state
    #[inline(always)]
    pub unsafe fn force_unlock(&self) {
        self.store(UNLOCKED)
    }

    /// Unlocks the state and changes it to the failed state
    #[inline(always)]
    pub unsafe fn terminate(&self) -> bool {
        self.v
            .compare_exchange(LOCKED, TERMINATED, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
    }

    /// Force terminates the signal, without checking for the current state of the signal
    #[inline(always)]
    pub unsafe fn force_terminate(&self) {
        self.store(TERMINATED)
    }

    /// Acquire lock for the current thread from the state, should not be used on any part of async because the thread that acquires
    ///  a lock should not go to sleep and we can't guarantee that in async
    #[inline(always)]
    pub fn lock(&self) -> bool {
        self.v
            .compare_exchange(UNLOCKED, LOCKED, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
    }

    /// Tries to upgrade the lock to starvation mode
    #[inline(always)]
    pub fn upgrade_lock(&self) -> bool {
        self.v
            .compare_exchange(
                LOCKED,
                LOCKED_STARVATION,
                Ordering::SeqCst,
                Ordering::Relaxed,
            )
            .is_ok()
    }
}
