use branches::likely;
#[cfg(not(loom))]
use lock_api::{GuardSend, RawMutex};

use crate::{
    backoff::*,
    primitives::{AtomicBool, Ordering},
};

pub struct RawMutexLock {
    locked: AtomicBool,
}

#[cfg_attr(loom, allow(dead_code))]
impl RawMutexLock {
    /// Creates an unlocked mutex. The loom build cannot use the lock_api
    /// `INIT` constant as loom atomics are not const-constructible.
    #[cfg(loom)]
    pub(crate) fn new() -> Self {
        RawMutexLock {
            locked: AtomicBool::new(false),
        }
    }
    #[inline(never)]
    fn lock_no_inline(&self) {
        // Test-and-test-and-set: spin on a plain load and only attempt the
        // CAS when the lock is observed free, so waiters do not bounce the
        // cache line between cores while the lock is held.
        spin_cond(|| !self.locked.load(Ordering::Relaxed) && self.try_lock());
    }
    #[inline(always)]
    pub(crate) fn lock(&self) {
        if likely(self.try_lock()) {
            return;
        }
        self.lock_no_inline();
    }
    #[inline(always)]
    pub(crate) fn try_lock(&self) -> bool {
        likely(
            self.locked
                .compare_exchange(
                    false,
                    true,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                )
                .is_ok(),
        )
    }
    #[inline(always)]
    pub(crate) unsafe fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

#[cfg(not(loom))]
unsafe impl RawMutex for RawMutexLock {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: RawMutexLock = RawMutexLock {
        locked: AtomicBool::new(false),
    };
    type GuardMarker = GuardSend;
    #[inline(always)]
    fn lock(&self) {
        RawMutexLock::lock(self)
    }

    #[inline(always)]
    fn try_lock(&self) -> bool {
        RawMutexLock::try_lock(self)
    }

    #[inline(always)]
    unsafe fn unlock(&self) {
        RawMutexLock::unlock(self)
    }
}
#[cfg(not(loom))]
#[allow(dead_code)]
pub type Mutex<T> = lock_api::Mutex<RawMutexLock, T>;
#[cfg(all(not(feature = "std-mutex"), not(loom)))]
pub type MutexGuard<'a, T> = lock_api::MutexGuard<'a, RawMutexLock, T>;

