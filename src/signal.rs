use crate::{backoff, pointer::KanalPtr};
use core::{
    cell::UnsafeCell,
    fmt::Debug,
    mem::ManuallyDrop,
    sync::atomic::{fence, AtomicUsize, Ordering},
    time::Duration,
};
#[cfg(feature = "async")]
use core::{mem::MaybeUninit, task::Waker};
use std::{
    thread::{current as current_thread, Thread},
    time::Instant,
};

use branches::{likely, unlikely};
use cacheguard::CacheGuard;

const UNINIT: usize = 0;
const LOCKED: usize = UNINIT + 1;
const LOCKED_STARVATION: usize = UNINIT + 2;
const TERMINATED: usize = !0 - 1;
const UNLOCKED: usize = !0;
#[cfg(feature = "async")]
const DONE: usize = usize::MAX / 2;

#[inline(always)]
#[cfg(feature = "async")]
fn tag_pointer<T>(ptr: *const T) -> *const () {
    debug_assert!(
        (ptr.addr() & 1) == 0,
        "Tagging pointer failed due to target memory alignment"
    );
    ptr.map_addr(|addr| addr | 1).cast()
}

#[inline(always)]
#[cfg(feature = "async")]
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
                PointerResult::Async(ptr)
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

pub(crate) struct SyncSignal<T> {
    state: CacheGuard<AtomicUsize>,
    ptr: KanalPtr<T>, //data: UnsafeCell<MaybeUninit<T>>,
    thread: UnsafeCell<ManuallyDrop<Thread>>,
    _pinned: core::marker::PhantomPinned,
}

impl<T> SyncSignal<T> {
    #[inline(always)]
    pub(crate) fn new(ptr: KanalPtr<T>) -> Self {
        Self {
            state: CacheGuard::new(AtomicUsize::new(LOCKED)),
            ptr,
            thread: UnsafeCell::new(ManuallyDrop::new(current_thread())),
            _pinned: core::marker::PhantomPinned,
        }
    }
    pub(crate) fn dynamic_ptr(&self) -> DynamicSignal<T> {
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
pub(crate) struct AsyncSignal<T> {
    state: CacheGuard<AtomicUsize>,
    data: UnsafeCell<MaybeUninit<T>>,
    waker: UnsafeCell<Waker>,
    _pinned: core::marker::PhantomPinned,
}

impl<T> Debug for AsyncSignal<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.debug_struct("AsyncSignal")
            .field("state", &self.state.load(Ordering::Relaxed))
            .finish()
    }
}

#[cfg(feature = "async")]
const fn no_op_waker() -> Waker {
    use core::task::{RawWaker, RawWakerVTable, Waker};

    const unsafe fn clone(_: *const ()) -> RawWaker {
        raw_waker()
    }
    const unsafe fn wake(_: *const ()) {}
    const unsafe fn wake_by_ref(_: *const ()) {}
    const unsafe fn drop(_: *const ()) {}

    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);
    const unsafe fn raw_waker() -> RawWaker {
        RawWaker::new(core::ptr::null(), &VTABLE)
    }

    unsafe { Waker::from_raw(raw_waker()) }
}

#[cfg(feature = "async")]
use crate::future::FutureState;
#[cfg(feature = "async")]
impl<T> AsyncSignal<T> {
    #[inline(always)]
    pub(crate) const fn new_recv() -> Self {
        Self {
            state: CacheGuard::new(AtomicUsize::new(Self::state_to_usize(
                FutureState::Unregistered,
            ))),
            data: UnsafeCell::new(MaybeUninit::uninit()),
            waker: UnsafeCell::new(no_op_waker()),
            _pinned: core::marker::PhantomPinned,
        }
    }

    #[inline(always)]
    pub(crate) const fn new_send(data: T) -> Self {
        Self {
            state: CacheGuard::new(AtomicUsize::new(Self::state_to_usize(
                FutureState::Unregistered,
            ))),
            data: UnsafeCell::new(MaybeUninit::new(data)),
            waker: UnsafeCell::new(no_op_waker()),
            _pinned: core::marker::PhantomPinned,
        }
    }

    /// SAFETY: caller must guarantee this signal is already finished and is not
    /// shared in any wait queue
    pub(crate) unsafe fn reset_send(&mut self, data: T) {
        self.data.get_mut().write(data);
        self.state.store(
            Self::state_to_usize(FutureState::Pending),
            Ordering::Release,
        );
    }

    #[inline(always)]
    pub(crate) const fn new_send_finished() -> Self {
        Self {
            state: CacheGuard::new(AtomicUsize::new(Self::state_to_usize(FutureState::Success))),
            data: UnsafeCell::new(MaybeUninit::uninit()),
            waker: UnsafeCell::new(no_op_waker()),
            _pinned: core::marker::PhantomPinned,
        }
    }

    #[inline(always)]
    const fn state_to_usize(state: FutureState) -> usize {
        match state {
            FutureState::Success => UNLOCKED,
            FutureState::Unregistered => UNINIT,
            FutureState::Pending => LOCKED,
            FutureState::Failure => TERMINATED,
            _ => DONE,
        }
    }

    #[inline(always)]
    const fn usize_to_state(val: usize) -> FutureState {
        match val {
            UNLOCKED => FutureState::Success,
            UNINIT => FutureState::Unregistered,
            LOCKED => FutureState::Pending,
            TERMINATED => FutureState::Failure,
            _ => FutureState::Done,
        }
    }

    #[inline(always)]
    pub(crate) fn state(&self) -> FutureState {
        Self::usize_to_state(self.state.load(Ordering::Acquire))
    }

    #[inline(always)]
    pub(crate) fn set_state(&self, state: FutureState) {
        self.state
            .store(Self::state_to_usize(state), Ordering::Release);
    }

    pub(crate) fn set_state_relaxed(&self, state: FutureState) {
        self.state
            .store(Self::state_to_usize(state), Ordering::Relaxed);
    }

    #[inline(always)]
    #[allow(unused)]
    pub(crate) fn compare_exchange_future_state(
        &self,
        current: FutureState,
        new: FutureState,
    ) -> Result<FutureState, FutureState> {
        match self.state.compare_exchange(
            Self::state_to_usize(current),
            Self::state_to_usize(new),
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(new),
            Err(v) => Err(Self::usize_to_state(v)),
        }
    }
    #[inline(always)]
    pub(crate) fn dynamic_ptr(&self) -> DynamicSignal<T> {
        DynamicSignal::new_async(self as *const AsyncSignal<T>)
    }
    #[inline(always)]
    #[cfg(feature = "async")]
    pub(crate) fn as_tagged_ptr(&self) -> *const () {
        tag_pointer(self as *const Self as *const ())
    }

    #[inline(always)]
    pub(crate) unsafe fn will_wake(&self, waker: &Waker) -> bool {
        (&*self.waker.get()).will_wake(waker)
    }
    // SAFETY: this function is only safe when owner of signal have exclusive lock
    // over channel,  this avoids another reader to clone the waker while we are
    // updating it.  this function should not be called if signal is
    // uninitialized or already shared.
    #[inline(always)]
    pub(crate) unsafe fn update_waker(&self, waker: &Waker) {
        *self.waker.get() = waker.clone();
    }
    #[inline(always)]
    pub(crate) unsafe fn clone_waker(&self) -> Waker {
        (&*self.waker.get()).clone()
    }
    #[inline(always)]
    pub(crate) unsafe fn write_data(this: *const Self, data: T) {
        let waker = (*this).clone_waker();
        (*this).data.get().write(MaybeUninit::new(data));
        (*this).state.store(UNLOCKED, Ordering::Release);
        waker.wake();
    }
    #[inline(always)]
    pub(crate) unsafe fn read_data(this: *const Self) -> T {
        let waker = (*this).clone_waker();
        let data = (*this).data.get().read().assume_init();
        (*this).state.store(UNLOCKED, Ordering::Release);
        waker.wake();
        data
    }

    // Drops waker without waking
    pub(crate) unsafe fn cancel(_this: *const Self) {}
    pub(crate) unsafe fn assume_init(&self) -> T {
        unsafe { self.data.get().read().assume_init() }
    }
    pub(crate) unsafe fn terminate(this: *const Self) {
        let waker = (*this).clone_waker();
        (*this).state.store(TERMINATED, Ordering::Release);
        waker.wake();
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
