use std::sync::atomic::{AtomicU8, Ordering};

/// The state keeps the state of signals in both sync and async to make eventing for senders and receivers possible
pub struct State {
    v: AtomicU8,
}

pub const UNLOCKED: u8 = 0;
pub const TERMINATED: u8 = 1;
pub const LOCKED: u8 = 2;
pub const LOCKED_STARVATION: u8 = 3;

impl State {
    #[inline(always)]
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

    /// Tries to change state from locked to new
    #[inline(always)]
    pub unsafe fn try_change(&self, new: u8) -> bool {
        self.v
            .compare_exchange(LOCKED, new, Ordering::Release, Ordering::Relaxed)
            .is_ok()
    }

    /// Force change state to the new state
    #[inline(always)]
    pub unsafe fn force(&self, new: u8) {
        self.v.store(new, Ordering::Release)
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
