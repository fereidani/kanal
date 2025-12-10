use crate::{backoff, pointer::KanalPtr};
#[cfg(feature = "async")]
use core::task::Waker;
use core::{
    cell::UnsafeCell,
    mem::{ManuallyDrop, MaybeUninit},
    sync::atomic::{fence, AtomicUsize, Ordering},
    time::Duration,
};
use std::{
    thread::{current as current_thread, Thread},
    time::Instant,
};

use branches::{likely, unlikely};

const LOCKED: usize = 0;
const LOCKED_STARVATION: usize = 1;
const TERMINATED: usize = !0 - 1;
const UNLOCKED: usize = !0;

#[inline(always)]
fn tag_pointer<T>(ptr: *const T) -> *const () {
    debug_assert!(
        (ptr.addr() & 1) == 0,
        "Tagging pointer failed due to target memory alignment"
    );
    ptr.map_addr(|addr| addr | 1).cast()
}

#[inline(always)]
fn untag_pointer<T>(ptr: *const ()) -> (*const T, bool) {
    let is_tagged = (ptr.addr() & 1) == 1;
    let untagged = ptr.map_addr(|addr| addr & !1).cast::<T>();
    (untagged, is_tagged)
}

#[repr(C)]
pub(crate) struct DynamicSignal<T> {
    ptr: *const (),
    _marker: core::marker::PhantomData<T>,
}

enum PointerResult<T> {
    Sync(*const SyncSignal<T>),
    #[cfg(feature = "async")]
    Async(*const AsyncSignal<T>),
}

impl<T> DynamicSignal<T> {
    #[cfg(feature = "async")]
    pub(crate) fn new_async(ptr: *const AsyncSignal<T>) -> Self {
        Self {
            ptr: tag_pointer(ptr as *const ()),
            _marker: core::marker::PhantomData,
        }
    }
    pub(crate) const fn new_sync(ptr: *const SyncSignal<T>) -> Self {
        Self {
            ptr: ptr as *const (),
            _marker: core::marker::PhantomData,
        }
    }
    pub(crate) fn eq_ptr(&self, ptr: *const ()) -> bool {
        core::ptr::eq(self.ptr, ptr)
    }
    #[inline(always)]
    fn resolve(&self) -> PointerResult<T> {
        #[cfg(feature = "async")]
        {
            let (ptr, tagged) = untag_pointer(self.ptr);
            if tagged {
                PointerResult::Async(ptr as *const AsyncSignal<T>)
            } else {
                PointerResult::Sync(ptr as *const SyncSignal<T>)
            }
        }
        #[cfg(not(feature = "async"))]
        {
            PointerResult::Sync(self.ptr as *const SyncSignal<T>)
        }
    }
    #[inline(always)]
    pub(crate) unsafe fn send(self, data: T) {
        match self.resolve() {
            PointerResult::Sync(ptr) => unsafe {
                SyncSignal::write_data(ptr, data);
            },
            #[cfg(feature = "async")]
            PointerResult::Async(ptr) => unsafe {
                AsyncSignal::write_data(ptr, data);
            },
        }
    }
    #[inline(always)]
    pub(crate) unsafe fn recv(self) -> T {
        match self.resolve() {
            PointerResult::Sync(ptr) => unsafe { SyncSignal::read_data(ptr) },
            #[cfg(feature = "async")]
            PointerResult::Async(ptr) => unsafe { AsyncSignal::read_data(ptr) },
        }
    }
    #[inline(always)]
    pub(crate) unsafe fn terminate(&self) {
        match self.resolve() {
            PointerResult::Sync(ptr) => unsafe {
                SyncSignal::terminate(ptr);
            },
            #[cfg(feature = "async")]
            PointerResult::Async(ptr) => unsafe {
                AsyncSignal::terminate(ptr);
            },
        }
    }
    #[inline(always)]
    pub(crate) unsafe fn cancel(&self) {
        match self.resolve() {
            PointerResult::Sync(ptr) => unsafe {
                SyncSignal::cancel(ptr);
            },
            #[cfg(feature = "async")]
            PointerResult::Async(ptr) => unsafe {
                AsyncSignal::cancel(ptr);
            },
        }
    }
}

pub struct SyncSignal<T> {
    state: AtomicUsize,
    ptr: KanalPtr<T>, //data: UnsafeCell<MaybeUninit<T>>,
    thread: UnsafeCell<ManuallyDrop<Thread>>,
    _pinned: core::marker::PhantomPinned,
}

impl<T> SyncSignal<T> {
    #[inline(always)]
    pub(crate) fn new(ptr: KanalPtr<T>) -> Self {
        Self {
            state: AtomicUsize::new(LOCKED),
            ptr,
            thread: UnsafeCell::new(ManuallyDrop::new(current_thread())),
            _pinned: core::marker::PhantomPinned,
        }
    }
    pub(crate) fn get_dynamic_ptr(&self) -> DynamicSignal<T> {
        DynamicSignal::new_sync(self as *const SyncSignal<T>)
    }
    #[inline(always)]
    pub(crate) fn as_tagged_ptr(&self) -> *const () {
        self as *const Self as *const ()
    }
    #[inline(always)]
    pub(crate) unsafe fn write_data(this: *const Self, data: T) {
        let thread = ManuallyDrop::take(&mut *(*this).thread.get());
        unsafe {
            (*this).ptr.write(data);
        }
        if unlikely((*this).state.swap(UNLOCKED, Ordering::AcqRel) == LOCKED_STARVATION) {
            thread.unpark();
        }
    }
    #[inline(always)]
    pub(crate) unsafe fn read_data(this: *const Self) -> T {
        let thread = ManuallyDrop::take(&mut *(*this).thread.get());
        let r = (*this).ptr.read();
        if unlikely((*this).state.swap(UNLOCKED, Ordering::AcqRel) == LOCKED_STARVATION) {
            thread.unpark();
        }
        r
    }
    pub(crate) fn is_terminated(&self) -> bool {
        self.state.load(Ordering::Acquire) == TERMINATED
    }
    pub(crate) unsafe fn assume_init(&self) -> T {
        self.ptr.read()
    }
    pub(crate) unsafe fn terminate(this: *const Self) {
        let thread = ManuallyDrop::take(&mut *(*this).thread.get());
        if unlikely((*this).state.swap(TERMINATED, Ordering::AcqRel) == LOCKED_STARVATION) {
            thread.unpark();
        }
    }
    pub(crate) unsafe fn cancel(this: *const Self) {
        let _ = ManuallyDrop::take(&mut *(*this).thread.get());
    }
    #[inline(always)]
    pub(crate) fn wait(&self) -> bool {
        let v = self.state.load(Ordering::Relaxed);
        if likely(v > LOCKED_STARVATION) {
            fence(Ordering::Acquire);
            return v == UNLOCKED;
        }
        let now = Instant::now();
        let spin_timeout = now.checked_add(Duration::from_micros(25)).unwrap_or(now);
        // 25 microseconds or 256 os yields, whichever happens first
        for _ in 0..4 {
            for _ in 0..64 {
                backoff::yield_os();
                let v = self.state.load(Ordering::Relaxed);
                if likely(v > LOCKED_STARVATION) {
                    fence(Ordering::Acquire);
                    return v == UNLOCKED;
                }
            }
            if unlikely(Instant::now() >= spin_timeout) {
                break;
            }
        }
        match self.state.compare_exchange(
            LOCKED,
            LOCKED_STARVATION,
            Ordering::Release,
            Ordering::Acquire,
        ) {
            Ok(_) => loop {
                std::thread::park();
                let v = self.state.load(Ordering::Relaxed);
                if likely(v > LOCKED_STARVATION) {
                    fence(Ordering::Acquire);
                    return v == UNLOCKED;
                }
            },
            Err(v) => v == UNLOCKED,
        }
    }
    /// Waits for the signal event in sync mode with a timeout
    pub(crate) fn wait_timeout(&self, until: Instant) -> bool {
        let v = self.state.load(Ordering::Relaxed);
        if likely(v > LOCKED_STARVATION) {
            fence(Ordering::Acquire);
            return v == UNLOCKED;
        }
        match self.state.compare_exchange(
            LOCKED,
            LOCKED_STARVATION,
            Ordering::Release,
            Ordering::Acquire,
        ) {
            Ok(_) => loop {
                let v = self.state.load(Ordering::Relaxed);
                if likely(v > LOCKED_STARVATION) {
                    fence(Ordering::Acquire);
                    return v == UNLOCKED;
                }
                let now = Instant::now();
                if now >= until {
                    return self.state.load(Ordering::Acquire) == UNLOCKED;
                }
                std::thread::park_timeout(until - now);
            },
            Err(v) => v == UNLOCKED,
        }
    }
}

#[cfg(feature = "async")]
pub struct AsyncSignal<T> {
    state: AtomicUsize,
    data: UnsafeCell<MaybeUninit<T>>,
    waker: MaybeUninit<Waker>,
    _pinned: core::marker::PhantomPinned,
}

#[cfg(feature = "async")]
pub(crate) enum AsyncSignalResult {
    Success,
    Failure,
    Pending,
}

#[cfg(feature = "async")]
impl<T> AsyncSignal<T> {
    #[inline(always)]
    pub(crate) const fn new_recv() -> Self {
        Self {
            state: AtomicUsize::new(LOCKED),
            data: UnsafeCell::new(MaybeUninit::uninit()),
            waker: MaybeUninit::uninit(),
            _pinned: core::marker::PhantomPinned,
        }
    }
    #[inline(always)]
    pub(crate) const fn new_send(data: T) -> Self {
        Self {
            state: AtomicUsize::new(LOCKED),
            data: UnsafeCell::new(MaybeUninit::new(data)),
            waker: MaybeUninit::uninit(),
            _pinned: core::marker::PhantomPinned,
        }
    }
    #[inline(always)]
    pub(crate) fn get_dynamic_ptr(&self) -> DynamicSignal<T> {
        DynamicSignal::new_async(self as *const AsyncSignal<T>)
    }
    #[inline(always)]
    pub(crate) fn as_tagged_ptr(&self) -> *const () {
        tag_pointer(self as *const Self as *const ())
    }
    #[inline(always)]
    pub(crate) unsafe fn will_wake(&self, waker: &Waker) -> bool {
        (&*self.waker.as_ptr()).will_wake(waker)
    }
    pub(crate) fn result(&self) -> AsyncSignalResult {
        let v = self.state.load(Ordering::Acquire);
        if likely(v == UNLOCKED) {
            AsyncSignalResult::Success
        } else if v == TERMINATED {
            AsyncSignalResult::Failure
        } else {
            AsyncSignalResult::Pending
        }
    }
    #[inline(always)]
    pub(crate) unsafe fn write_data(this: *const Self, data: T) {
        let waker = (*this).waker.as_ptr().read();
        (*this).data.get().write(MaybeUninit::new(data));
        (*this).state.store(UNLOCKED, Ordering::Release);
        waker.wake();
    }
    #[inline(always)]
    pub(crate) unsafe fn read_data(this: *const Self) -> T {
        let waker = (*this).waker.as_ptr().read();
        let data = (*this).data.get().read().assume_init();
        (*this).state.store(UNLOCKED, Ordering::Release);
        waker.wake();
        data
    }
    pub(crate) unsafe fn cancel(this: *const Self) {
        let _ = (*this).waker.as_ptr().read();
    }
    pub(crate) unsafe fn assume_init(&self) -> T {
        unsafe { self.data.get().read().assume_init() }
    }
    pub(crate) unsafe fn terminate(this: *const Self) {
        let waker = (*this).waker.as_ptr().read();
        (*this).state.store(TERMINATED, Ordering::Release);
        waker.wake();
    }
    #[inline(always)]
    pub(crate) unsafe fn init_waker(&mut self, waker: Waker) {
        self.waker.as_mut_ptr().write(waker);
    }
    #[inline(always)]
    pub(crate) unsafe fn register_waker(&mut self, waker: Waker) {
        let waker_ptr = self.waker.as_mut_ptr();
        waker_ptr.drop_in_place();
        waker_ptr.write(waker);
    }
    #[inline(always)]
    pub(crate) unsafe fn drop_data(&mut self) {
        let ptr = self.data.get();
        (&mut *ptr).as_mut_ptr().drop_in_place();
    }
    #[inline(never)]
    #[cold]
    pub(crate) fn blocking_wait(&self) -> bool {
        let v = self.state.load(Ordering::Relaxed);
        if likely(v > LOCKED_STARVATION) {
            fence(Ordering::Acquire);
            return v == UNLOCKED;
        }

        for _ in 0..32 {
            backoff::yield_os();
            let v = self.state.load(Ordering::Relaxed);
            if likely(v > LOCKED_STARVATION) {
                fence(Ordering::Acquire);
                return v == UNLOCKED;
            }
        }

        // Usually this part will not happen but you can't be sure
        let mut sleep_time: u64 = 1 << 10;
        loop {
            backoff::sleep(Duration::from_nanos(sleep_time));
            let v = self.state.load(Ordering::Relaxed);
            if likely(v > LOCKED_STARVATION) {
                fence(Ordering::Acquire);
                return v == UNLOCKED;
            }
            // increase sleep_time gradually to 262 microseconds
            if sleep_time < (1 << 18) {
                sleep_time <<= 1;
            }
        }
    }
}
