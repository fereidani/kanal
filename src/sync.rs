/// This module provides a synchronization mechanism using the futex API of the underlying operating system.
///  It is used as an alternative to `std::thread::park` in order to improve performance for high-throughput channels.
/// The module includes APIs for waiting and waking that have a low initialization cost. Implementations are provided
///  for major operating systems, and in cases where an implementation is not available, the module falls back to using `std::thread::park`.
// TODO: Implement support for Windows. To do this, both the `WaitOnAddress` API and `KeyedEvent` should be implemented.
//   Note that the availability of the WaitOnAddress API should be checked only once and saved in a static variable.
// TODO: Implement support for MacOS. It may be possible to use `os_unfair_lock_lock` and `os_unfair_lock_unlock`,
//   but further investigation into the availability and performance of these APIs is needed.
pub use imp::*;

pub trait WaitAPI {
    fn wait(&self);
    fn wake(&self);
    fn new() -> Self;
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    use super::WaitAPI;
    use std::sync::atomic::AtomicU32;
    //       long syscall(SYS_futex, uint32_t *uaddr, int futex_op, uint32_t val,
    //                const struct timespec *timeout,   /* or: uint32_t val2 */
    //                uint32_t *uaddr2, uint32_t val3);
    #[inline(always)]
    fn futex_wait(futex: &AtomicU32, expected: i32) -> bool {
        unsafe {
            // Safety: futex pointer address is valid, so calling this syscall is safe
            libc::syscall(
                libc::SYS_futex,
                futex as *const AtomicU32 as *mut u32,
                libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG,
                expected,
                std::ptr::null_mut::<()>(),
            ) != -1
        }
    }
    // returns false if action fails
    #[inline(always)]
    fn futex_wake(futex: &AtomicU32) -> bool {
        // Safety: futex pointer address is valid, so calling this syscall is safe
        unsafe {
            libc::syscall(
                libc::SYS_futex,
                futex as *const AtomicU32 as *mut u32,
                libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG,
                1,
            ) > 0
        }
    }
    pub struct SysWait {
        futex: AtomicU32,
    }
    impl WaitAPI for SysWait {
        #[inline(always)]
        fn wait(&self) {
            futex_wait(&self.futex, 0);
        }

        #[inline(always)]
        fn wake(&self) {
            while !futex_wake(&self.futex) {}
        }

        #[inline(always)]
        fn new() -> Self {
            SysWait { futex: 0.into() }
        }
    }
}

#[cfg(target_os = "openbsd")]
mod imp {
    use super::WaitAPI;
    use std::sync::atomic::AtomicU32;
    #[inline(always)]
    fn futex_wait(futex: &AtomicU32, expected: i32) {
        unsafe {
            libc::futex(
                futex as *const AtomicU32 as *mut _,
                libc::FUTEX_WAIT,
                expected,
                std::ptr::null(),
                std::ptr::null_mut(),
            );
        }
    }
    // returns false if action fails
    #[inline(always)]
    fn futex_wake(futex: &AtomicU32) -> bool {
        unsafe {
            libc::futex(
                futex as *const AtomicU32 as *mut _,
                libc::FUTEX_WAKE,
                1,
                std::ptr::null(),
                std::ptr::null_mut(),
            ) > 0
        }
    }

    pub struct SysWait {
        futex: AtomicU32,
    }

    impl WaitAPI for SysWait {
        #[inline(always)]
        fn wait(&self) {
            futex_wait(&self.futex, 0i32);
        }

        #[inline(always)]
        fn wake(&self) {
            while !futex_wake(&self.futex) {}
        }

        #[inline(always)]
        fn new() -> Self {
            SysWait { futex: 0.into() }
        }
    }
}

#[cfg(target_os = "emscripten")]
mod imp {
    use super::WaitAPI;
    use std::sync::atomic::AtomicU32;

    extern "C" {
        fn emscripten_futex_wake(addr: *const AtomicU32, count: libc::c_int) -> libc::c_int;
        fn emscripten_futex_wait(
            addr: *const AtomicU32,
            val: libc::c_uint,
            max_wait_ms: libc::c_double,
        ) -> libc::c_int;
    }

    #[inline(always)]
    fn futex_wait(futex: &AtomicU32, expected: libc::c_uint) {
        unsafe {
            emscripten_futex_wait(futex as *const AtomicU32, expected, f64::INFINITY);
        }
    }
    // returns false if action fails
    #[inline(always)]
    fn futex_wake(futex: &AtomicU32) -> bool {
        unsafe { emscripten_futex_wake(futex as *const AtomicU32, 1) > 0 }
    }

    pub struct SysWait {
        futex: AtomicU32,
    }

    impl WaitAPI for SysWait {
        #[inline(always)]
        fn wait(&self) {
            futex_wait(&self.futex, 0i32);
        }

        #[inline(always)]
        fn wake(&self) {
            while !futex_wake(&self.futex) {}
        }

        #[inline(always)]
        fn new() -> Self {
            SysWait { futex: 0.into() }
        }
    }
}

#[cfg(target_os = "freebsd")]
mod imp {
    use super::WaitAPI;
    use std::sync::atomic::{AtomicU32, Ordering};

    pub fn futex_wake(futex: &AtomicU32) {
        unsafe {
            futex.store(!0, Ordering::Relaxed);
            libc::_umtx_op(
                futex as *const AtomicU32 as *mut _,
                libc::UMTX_OP_WAKE_PRIVATE,
                1 as libc::c_ulong,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
    }
    pub fn futex_wait(futex: &AtomicU32, expected: libc::c_ulong) {
        while {
            unsafe {
                libc::_umtx_op(
                    futex as *const AtomicU32 as *mut _,
                    libc::UMTX_OP_WAIT_UINT_PRIVATE,
                    expected,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
            }
            futex.load(Ordering::Relaxed) == 0
        } {}
    }

    pub struct SysWait {
        futex: AtomicU32,
    }

    impl WaitAPI for SysWait {
        #[inline(always)]
        fn wait(&self) {
            futex_wait(&self.futex, 0);
        }

        #[inline(always)]
        fn wake(&self) {
            futex_wake(&self.futex);
        }

        #[inline(always)]
        fn new() -> Self {
            SysWait { futex: 0.into() }
        }
    }
}

#[cfg(target_os = "dragonfly")]
mod imp {
    use super::WaitAPI;
    use std::sync::atomic::{AtomicU32, Ordering};

    pub fn futex_wake(futex: &AtomicU32) {
        unsafe {
            futex.store(!0, Ordering::Relaxed);
            unsafe {
                libc::umtx_wakeup(futex as *const AtomicU32 as *const _, 1);
            };
        }
    }
    pub fn futex_wait(futex: &AtomicU32, expected: libc::c_int) {
        while {
            unsafe {
                libc::umtx_sleep(futex as *const AtomicU32 as *const _, expected, 0);
            }
            futex.load(Ordering::Relaxed) == 0
        } {}
    }

    pub struct SysWait {
        futex: AtomicU32,
    }

    impl WaitAPI for SysWait {
        #[inline(always)]
        fn wait(&self) {
            futex_wait(&self.futex, 0);
        }

        #[inline(always)]
        fn wake(&self) {
            futex_wake(&self.futex);
        }

        #[inline(always)]
        fn new() -> Self {
            SysWait { futex: 0.into() }
        }
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "openbsd",
    target_os = "freebsd",
    target_os = "dragonbsd",
    target_os = "emscripten",
)))]
mod imp {
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        thread::Thread,
    };

    use super::WaitAPI;

    pub struct SysWait {
        thread: Thread,
        waiting: AtomicBool,
    }

    impl WaitAPI for SysWait {
        #[inline(always)]
        fn new() -> Self {
            Self {
                thread: std::thread::current(),
                waiting: true.into(),
            }
        }

        #[inline(always)]
        fn wait(&self) {
            while {
                std::thread::park();
                self.waiting.load(Ordering::Relaxed) == true
            } {}
        }

        #[inline(always)]
        fn wake(&self) {
            self.waiting.store(false, Ordering::Relaxed);
            self.thread.unpark();
        }
    }
}
