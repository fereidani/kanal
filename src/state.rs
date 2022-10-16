use std::{
    sync::atomic::{AtomicU8, Ordering},
    time::{Duration, SystemTime},
};

/// State keeps state of signals in both sync and async to make eventing for senders and receivers possible
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
    /// Returns current value of State with recommended Ordering
    #[inline(always)]
    pub fn value(&self) -> u8 {
        self.v.load(Ordering::SeqCst)
    }

    /// Stores value with recommended Ordering in State
    #[inline(always)]
    pub fn store(&self, v: u8) {
        self.v.store(v, Ordering::SeqCst)
    }

    #[inline(always)]
    pub fn wait_unlock_until(&self, until: SystemTime) -> u8 {
        let v = self.v.load(Ordering::SeqCst);
        if v != LOCKED {
            return v;
        }
        while SystemTime::now() < until {
            for _ in 0..(1 << 10) {
                let v = self.v.load(Ordering::SeqCst);
                if v != LOCKED {
                    return v;
                }
                //spin_loop();
                std::thread::yield_now();
            }
            let remaining_time = until.duration_since(SystemTime::now()).unwrap_or_default();
            if remaining_time.is_zero() {
                break;
            }
            std::thread::yield_now();
        }
        self.v.load(Ordering::SeqCst)
    }

    #[inline(always)]
    pub fn wait_indefinitely(&self) -> u8 {
        let v = self.v.load(Ordering::SeqCst);
        if v != LOCKED {
            return v;
        }
        let mut sleep_time: u64 = 1 << 3;
        loop {
            for _ in 0..(1 << 8) {
                let v = self.v.load(Ordering::SeqCst);
                if v != LOCKED {
                    return v;
                }
                //spin_loop();
                std::thread::yield_now();
            }
            std::thread::sleep(Duration::from_nanos(sleep_time));
            // increase sleep_time gradually to 262 microseconds
            if sleep_time < (1 << 18) {
                sleep_time <<= 1;
            }
        }
    }
    /// Unlocks the state and change it to succesfull state
    #[inline(always)]
    pub unsafe fn unlock(&self) -> bool {
        self.v
            .compare_exchange(LOCKED, UNLOCKED, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
    }

    #[inline(always)]
    pub unsafe fn force_unlock(&self) {
        self.store(UNLOCKED)
    }

    /// Unlocks the state and change it to failed state
    #[inline(always)]
    pub unsafe fn terminate(&self) -> bool {
        self.v
            .compare_exchange(LOCKED, TERMINATED, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
    }

    #[inline(always)]
    pub unsafe fn force_terminate(&self) {
        self.store(TERMINATED)
    }

    /// acquire lock for current thread from state, should not be used on any part of async because thread that acquire
    ///  a lock should not go to sleep and we can't guarantee that in async
    #[inline(always)]
    pub fn lock(&self) -> bool {
        self.v
            .compare_exchange(UNLOCKED, LOCKED, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
    }

    /// tries to upgrade the lock to starvation mode
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
