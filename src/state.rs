#[cfg(feature = "async")]
use std::time::Duration;
use std::{
    sync::atomic::{fence, AtomicU8, Ordering},
    time::Instant,
};

use crate::backoff;

/// The state keeps the state of signals in both sync and async to make eventing for senders and receivers possible
pub struct State {
    v: AtomicU8,
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

    /// Returns the current value of State with the relaxed ordering
    #[inline(always)]
    pub fn relaxed(&self) -> u8 {
        self.v.load(Ordering::Relaxed)
    }

    /// Returns the current value of State with the acquire ordering
    #[inline(always)]
    pub fn acquire(&self) -> u8 {
        self.v.load(Ordering::Acquire)
    }

    /// Waits synchronously without putting the thread to sleep until the instant time is reached
    /// this function may return with latency after instant time because of spin loop implementation
    #[inline(always)]
    #[must_use = "ignoring wait functions return value will lead to UB"]
    pub fn wait_unlock_until(&self, until: Instant) -> u8 {
        for _ in 0..32 {
            let v = self.v.load(Ordering::Relaxed);
            if v < LOCKED {
                fence(Ordering::Acquire);
                return v;
            }
            // randomize next entry with yield_now
            backoff::yield_now();
        }
        //return self.v.load(Ordering::Acquire);
        while Instant::now() < until {
            let v = self.v.load(Ordering::Relaxed);
            if v < LOCKED {
                fence(Ordering::Acquire);
                return v;
            }
            backoff::yield_now_std();
        }
        self.v.load(Ordering::Acquire)
    }

    /// Waits synchronously for the signal in sync mode, it should not be used anywhere except a drop of async future
    #[cfg(feature = "async")]
    #[must_use = "ignoring wait functions return value will lead to UB"]
    pub fn wait_indefinitely(&self) -> u8 {
        let v = self.relaxed();
        if v < LOCKED {
            fence(Ordering::Acquire);
            return v;
        }

        for _ in 0..32 {
            //backoff::spin_wait(96);
            backoff::yield_now_std();
            let v = self.relaxed();
            if v < LOCKED {
                fence(Ordering::Acquire);
                return v;
            }
        }

        let mut sleep_time: u64 = 1 << 10;
        loop {
            backoff::sleep(Duration::from_nanos(sleep_time));
            let v = self.relaxed();
            if v < LOCKED {
                fence(Ordering::Acquire);
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
            .compare_exchange(LOCKED, UNLOCKED, Ordering::Release, Ordering::Relaxed)
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
            .compare_exchange(LOCKED, TERMINATED, Ordering::Release, Ordering::Relaxed)
            .is_ok()
    }

    /// Force terminates the signal, without checking for the current state of the signal
    #[inline(always)]
    pub unsafe fn force_terminate(&self) {
        self.v.store(TERMINATED, Ordering::Release)
    }

    /// Tries to upgrade the lock to starvation mode
    #[inline(always)]
    pub fn upgrade_lock(&self) -> bool {
        self.v
            .compare_exchange(
                LOCKED,
                LOCKED_STARVATION,
                Ordering::Release,
                Ordering::Relaxed,
            )
            .is_ok()
    }
}
