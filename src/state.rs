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
    #[inline]
    pub const fn locked() -> Self {
        Self {
            v: AtomicU8::new(LOCKED),
        }
    }

    /// Returns the current value of State with recommended Ordering
    #[inline(always)]
    pub fn value(&self, ordering: Ordering) -> u8 {
        self.v.load(ordering)
    }

    /// Waits synchronously without putting the thread to sleep until the instant time is reached
    /// this function may return with latency after instant time because of spin loop implementation
    #[inline(always)]
    #[must_use = "ignoring wait functions return value will lead to UB"]
    pub fn wait_unlock_until(&self, until: Instant) -> u8 {
        for _ in 0..(1 << 8) {
            let v = self.v.load(Ordering::Acquire);
            if v < LOCKED {
                return v;
            }
            backoff::spin_hint();
        }
        while Instant::now() < until {
            let v = self.v.load(Ordering::Acquire);
            if v < LOCKED {
                return v;
            }
            backoff::yield_now();
        }
        self.v.load(Ordering::Acquire)
    }

    /// Waits synchronously for the signal in sync mode, it should not be used anywhere except a drop of async future
    #[cfg(feature = "async")]
    #[must_use = "ignoring wait functions return value will lead to UB"]
    pub fn wait_indefinitely(&self) -> u8 {
        for _ in 0..(1 << 8) {
            let v = self.v.load(Ordering::Acquire);
            if v < LOCKED {
                return v;
            }
            backoff::spin_hint();
        }
        let mut sleep_time: u64 = 1 << 10;
        loop {
            backoff::sleep(Duration::from_nanos(sleep_time));
            let v = self.v.load(Ordering::Acquire);
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
        // TODO: investigate whether we can weaken the internal orderings even more.
        self.v
            .compare_exchange(LOCKED, UNLOCKED, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    /// Force unlocks the state without checking for the current state
    #[inline(always)]
    pub unsafe fn force_unlock(&self) {
        self.v.store(UNLOCKED, Ordering::Release)
    }

    /// Unlocks the state and changes it to the failed state
    #[inline(always)]
    pub unsafe fn terminate(&self) -> bool {
        self.v
            .compare_exchange(LOCKED, TERMINATED, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    /// Force terminates the signal, without checking for the current state of the signal
    #[inline(always)]
    pub unsafe fn force_terminate(&self) {
        self.v.store(TERMINATED, Ordering::Release)
    }

    /// Acquire lock for the current thread from the state, should not be used on any part of async because the thread that acquires
    ///  a lock should not go to sleep and we can't guarantee that in async
    #[inline(always)]
    pub fn lock(&self) -> bool {
        self.v
            .compare_exchange(UNLOCKED, LOCKED, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    /// Tries to upgrade the lock to starvation mode
    #[inline(always)]
    pub fn upgrade_lock(&self) -> bool {
        self.v
            .compare_exchange(
                LOCKED,
                LOCKED_STARVATION,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .is_ok()
    }

    /// Returns whether the state is still in locked modes
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.v.load(Ordering::Acquire) >= LOCKED
    }

    /// Returns whether the state is terminated by the a successful call to the`terminate` or any call to the `force_terminate` function or not
    #[inline]
    pub fn is_terminated(&self) -> bool {
        self.v.load(Ordering::Acquire) == TERMINATED
    }
}
