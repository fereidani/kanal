use lock_api::{GuardSend, RawMutex};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

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

        let mut sleep_time: u64 = 1 << 5;
        let mut cycles: u32 = 1 << 12;

        loop {
            if sleep_time < (1 << 19) {
                for _ in 0..1 << 12 {
                    if self.try_lock() {
                        return;
                    }
                    //spin_loop();
                    std::thread::yield_now();
                }
            } else {
                if cycles < (1 << 31) {
                    cycles <<= 1;
                }
                for _ in 0..cycles {
                    if self.try_lock() {
                        return;
                    }
                    //spin_loop();
                    std::thread::yield_now();
                }
            }
            std::thread::sleep(Duration::from_nanos(sleep_time));
            if sleep_time < (1 << 20) {
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

pub type Mutex<T> = lock_api::Mutex<RawMutexLock, T>;
pub type MutexGuard<'a, T> = lock_api::MutexGuard<'a, RawMutexLock, T>;
