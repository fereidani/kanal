use lock_api::{GuardSend, RawMutex};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use crate::backoff;

const INTIIAL_SPIN_CYCLES: usize = 1 << 3;
const STRATEGY_SWITCH_THRESHOLD: usize = 4;

pub struct RawMutexLock {
    locked: AtomicBool,
}

unsafe impl RawMutex for RawMutexLock {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: RawMutexLock = RawMutexLock {
        locked: AtomicBool::new(false),
    };
    type GuardMarker = GuardSend;
    #[inline(always)]
    fn lock(&self) {
        if self.try_lock() {
            return;
        }
        for _ in 0..STRATEGY_SWITCH_THRESHOLD {
            for _ in 0..INTIIAL_SPIN_CYCLES {
                if self.try_lock() {
                    return;
                }
                backoff::spin_hint();
            }
            // randomize next entry with yield_now
            backoff::yield_now();
        }
        let mut cycles = INTIIAL_SPIN_CYCLES << 1;
        loop {
            // Backoff about 0.5ms and try harder next time
            backoff::sleep(Duration::from_nanos(backoff::randomize(1 << 19) as u64));
            for _ in 0..cycles {
                if self.try_lock() {
                    return;
                }
                backoff::spin_hint();
            }
            // Eventual Fairness: Increase spin cycles by multipling it by 2, this gives better chance to long waiting threads to acquire Mutex
            if cycles < (1 << 31) {
                cycles <<= 1;
            }
        }
    }

    #[inline(always)]
    fn try_lock(&self) -> bool {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    #[inline(always)]
    unsafe fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}
#[allow(dead_code)]
pub type Mutex<T> = lock_api::Mutex<RawMutexLock, T>;
#[cfg(not(feature = "std-mutex"))]
pub type MutexGuard<'a, T> = lock_api::MutexGuard<'a, RawMutexLock, T>;
