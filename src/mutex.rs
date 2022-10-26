use lock_api::{GuardSend, RawMutex};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub struct RawMutexLock {
    locked: AtomicBool,
}

const INITIAL_SLEEP_TIME: usize = 1 << 4;
const INTIIAL_SPIN_CYCLES: usize = 1 << 12;
const STRATEGY_SWITCH_THRESHOLD: usize = INITIAL_SLEEP_TIME << 8;
const MAX_BACKOFF_TIME: usize = 1 << 18; // about 0.26ms

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
        let mut sleep_time = INITIAL_SLEEP_TIME;
        let mut cycles = INTIIAL_SPIN_CYCLES;
        loop {
            if sleep_time < STRATEGY_SWITCH_THRESHOLD {
                for _ in 0..INITIAL_SLEEP_TIME {
                    if self.try_lock() {
                        return;
                    }
                    std::thread::yield_now();
                    //std::hint::spin_loop();
                }
            } else {
                // Eventual Fairness: Increase spin cycles by a factor of 2, this gives better chance to long waiting threads to acquire Mutex
                if cycles < (1 << 31) {
                    cycles <<= 1;
                }
                for _ in 0..cycles {
                    if self.try_lock() {
                        return;
                    }
                    std::thread::yield_now();
                    //std::hint::spin_loop();
                }
            }
            std::thread::sleep(Duration::from_nanos(sleep_time as u64));
            // Increase backoff time by a factor of 2 until we reach maximum backoff time
            if sleep_time < MAX_BACKOFF_TIME {
                sleep_time <<= 1
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
