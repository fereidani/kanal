use std::{
    hint::spin_loop,
    sync::atomic::{AtomicU8, Ordering},
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

impl State {
    /// Checks current state is in locked mode
    #[inline(always)]
    pub fn is_locked(&self) -> bool {
        self.v.load(Ordering::SeqCst) == 1
    }

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

    /// Waits for lock in a spin loop and returns last lock value
    #[inline(always)]
    pub fn wait_unlock(&self) -> u8 {
        while self.is_locked() {
            spin_loop();
        }
        self.v.load(Ordering::SeqCst)
    }

    /// Unlocks the state and change it to succesfull state
    #[inline(always)]
    pub unsafe fn unlock(&self) -> bool {
        self.v
            .compare_exchange(1, 0, Ordering::Release, Ordering::Relaxed)
            .is_ok()
    }
    /// Unlocks the state and change it to failed state
    #[inline(always)]
    pub unsafe fn terminate(&self) -> bool {
        self.v
            .compare_exchange(1, 2, Ordering::Release, Ordering::Relaxed)
            .is_ok()
    }
    /// acquire lock for current thread from state, should not be used on any part of async because thread that acquire
    ///  a lock should not go to sleep and we can't guarantee that in async
    #[inline(always)]
    pub fn lock(&self) -> bool {
        self.v
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }
}
