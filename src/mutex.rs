use lock_api::{GuardSend, RawMutex};
use std::{
    hint::spin_loop,
    sync::atomic::{AtomicBool, Ordering},
};

pub struct RawMutexLock(AtomicBool);

unsafe impl RawMutex for RawMutexLock {
    const INIT: RawMutexLock = RawMutexLock(AtomicBool::new(false));
    type GuardMarker = GuardSend;

    #[inline(always)]
    fn lock(&self) {
        while !self.try_lock() {
            spin_loop()
        }
    }

    #[inline(always)]
    fn try_lock(&self) -> bool {
        self.0
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    #[inline(always)]
    unsafe fn unlock(&self) {
        self.0.store(false, Ordering::Release);
    }
}

pub type Mutex<T> = lock_api::Mutex<RawMutexLock, T>;
//pub type MutexGuard<'a, T> = lock_api::MutexGuard<'a, RawMutexLock, T>;
