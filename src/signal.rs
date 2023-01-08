use crate::backoff;
use crate::pointer::KanalPtr;
use crate::state::{State, LOCKED, TERMINATED, UNLOCKED};
use crate::sync::{SysWait, WaitAPI};
use std::sync::atomic::{fence, Ordering};
#[cfg(feature = "async")]
use std::task::{Poll, Waker};
use std::time::Instant;

#[cfg(feature = "async")]
pub struct AsyncSignal<T> {
    state: State,
    ptr: KanalPtr<T>,
    waker: Option<Waker>,
}

#[cfg(feature = "async")]
unsafe impl<T> Send for AsyncSignal<T> {}

#[cfg(feature = "async")]
impl<T> std::future::Future for AsyncSignal<T> {
    type Output = u8;
    #[inline(always)]
    fn poll(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<Self::Output> {
        let v = self.state.relaxed();
        if v >= LOCKED {
            Poll::Pending
        } else {
            fence(Ordering::Acquire);
            Poll::Ready(v)
        }
    }
}

#[cfg(feature = "async")]
impl<T> AsyncSignal<T> {
    /// Signal to send data to a writer
    #[inline(always)]
    pub fn new() -> Self {
        Self {
            state: State::locked(),
            ptr: Default::default(),
            waker: Default::default(),
        }
    }
    /// Signal to send data to a writer for specific kanal pointer
    #[inline(always)]
    pub(crate) fn new_inside_ptr(ptr: KanalPtr<T>) -> Self {
        Self {
            state: State::locked(),
            ptr,
            waker: Default::default(),
        }
    }
    /// Set pointer to data for receiving or sending
    #[inline(always)]
    pub(crate) fn set_ptr(&mut self, ptr: KanalPtr<T>) {
        self.ptr = ptr;
    }

    /// Drops data inside pointer
    #[inline(always)]
    pub unsafe fn read_and_drop_ptr(&self) {
        let o = self.ptr.read();
        drop(o);
    }

    /// Read data from kanal ptr. should not be called from anyone but creator of signal.
    #[inline(always)]
    pub unsafe fn read_kanal_ptr(&self) -> T {
        self.ptr.read()
    }

    /// Convert async signal to common signal that works with channel internal
    #[inline(always)]
    pub fn as_signal(&self) -> Signal<T> {
        Signal::Async(self as *const Self)
    }

    /// Sends object to the signal
    /// Safety: it's only safe to call on receive signals that are not terminated
    #[inline(always)]
    pub unsafe fn send(this: *const Self, d: T) {
        (*this).ptr.write(d);
        let waker = AsyncSignal::clone_waker(this);
        (*this).state.force_unlock();
        waker.wake();
    }

    /// Receives object from signal
    /// Safety: it's only safe to call on send signals that are not terminated
    #[inline(always)]
    pub unsafe fn recv(this: *const Self) -> T {
        let waker = AsyncSignal::clone_waker(this);
        let r = (*this).ptr.read();
        (*this).state.force_unlock();
        waker.wake();
        r
    }

    /// Terminates operation and notifies the waiter , shall not be called more than once
    /// Safety: it's only safe to be called only once on send/receive signals that are not finished or terminated
    #[inline(always)]
    pub unsafe fn terminate(this: *const Self) {
        let waker = AsyncSignal::clone_waker(this);
        (*this).state.force_terminate();
        waker.wake();
    }

    /// Loads pointer data and drops it in place
    /// Safety: it should only be used once, and only when data in ptr is valid and not moved.
    #[inline(always)]
    pub unsafe fn load_and_drop(this: *const Self) {
        let _ = (*this).ptr.read();
    }

    /// Waits for signal and returns true if send/recv operation was successful
    pub fn wait_indefinitely(&self) -> u8 {
        self.state.wait_indefinitely()
    }

    /// Register waker for async
    #[cfg(feature = "async")]
    #[inline(always)]
    pub fn register(&mut self, waker: &Waker) {
        self.waker = Some(waker.clone())
    }

    /// Checks if provided waker wakes the same task
    #[cfg(feature = "async")]
    #[inline(always)]
    pub fn will_wake(&self, waker: &Waker) -> bool {
        self.waker.as_ref().unwrap().will_wake(waker)
    }

    /// clones waker from signal pointer
    /// Safety: only safe to call if signal is on waiting state.
    #[inline(always)]
    #[cfg(feature = "async")]
    unsafe fn clone_waker(this: *const Self) -> Waker {
        (*this).waker.as_ref().unwrap().clone()
    }
}

pub struct SyncSignal<T> {
    state: State,
    ptr: KanalPtr<T>,
    os_signal: SysWait,
}

unsafe impl<T> Send for SyncSignal<T> {}
unsafe impl<T> Sync for SyncSignal<T> {}

#[allow(dead_code)]
impl<T> SyncSignal<T> {
    /// Returns new sync signal for the provided thread
    #[inline(always)]
    pub(crate) fn new(ptr: KanalPtr<T>) -> Self {
        Self {
            state: State::locked(),
            ptr,
            os_signal: SysWait::new(),
        }
    }

    /// Drops data inside kanal pointer
    #[inline(always)]
    pub unsafe fn read_and_drop_ptr(&mut self) {
        let o = self.ptr.read();
        drop(o);
    }

    /// Convert sync signal to common signal that works with channel internal
    pub fn as_signal(&self) -> Signal<T> {
        Signal::Sync(self as *const Self)
    }

    /// Writes data to the pointer, shall not be called more than once
    /// has to be done through a pointer because by the end of this scope
    /// the object might have been destroyed by the owning receiver
    /// Safety: it's only safe to call on receive signals that are not terminated
    #[inline(always)]
    pub unsafe fn send(this: *const Self, d: T) {
        (*this).ptr.write(d);
        if !(*this).state.unlock() {
            (*this).state.force_unlock();
            (*this).os_signal.wake();
        }
    }

    /// Read data from the pointer, shall not be called more than once
    /// has to be done through a pointer because by the end of this scope
    /// the object might have been destroyed by the owning receiver
    /// Safety: it's only safe to call on send signals that are not terminated
    #[inline(always)]
    pub unsafe fn recv(this: *const Self) -> T {
        let d = (*this).ptr.read();
        if !(*this).state.unlock() {
            (*this).state.force_unlock();
            (*this).os_signal.wake();
        }
        d
    }

    /// Assumes data inside self.ptr is correct and reads it.
    #[inline(always)]
    pub unsafe fn assume_init(&self) -> T {
        self.ptr.read()
    }

    /// Loads pointer data and drops it in place
    /// Safety: it should only be used once, and only when data in ptr is valid and not moved.
    #[inline(always)]
    pub unsafe fn load_and_drop(this: *const Self) {
        let _ = (*this).ptr.read();
    }

    /// Terminates operation and notifies the waiter , shall not be called more than once
    /// has to be done through a pointer because by the end of this scope
    /// the object might have been destroyed by the owner
    /// Safety: it's only safe to be called only once on send/receive signals that are not finished or terminatedv
    #[inline(always)]
    pub unsafe fn terminate(this: *const Self) {
        if !(*this).state.terminate() {
            (*this).state.force_terminate();
            (*this).os_signal.wake();
        }
    }

    /// Waits for signal and returns true if send/recv operation was successful
    #[inline(always)]
    pub fn wait(&self) -> bool {
        let v = self.state.relaxed();
        if v < LOCKED {
            fence(Ordering::Acquire);
            return v == UNLOCKED;
        }

        for _ in 0..256 {
            //backoff::spin_wait(96);
            backoff::yield_now_std();
            let v = self.state.relaxed();
            if v < LOCKED {
                fence(Ordering::Acquire);
                return v == UNLOCKED;
            }
        }

        if self.state.upgrade_lock() {
            self.os_signal.wait();
        }
        self.state.acquire() == UNLOCKED
    }

    /// Waits for signal and returns true if send/recv operation was successful
    #[inline(always)]
    pub fn wait_timeout(&self, until: Instant) -> bool {
        let v = self.state.wait_unlock_until(until);
        fence(Ordering::Acquire);
        v == UNLOCKED
    }

    /// Returns whether the signal is terminated by the `terminate` function or not
    #[inline(always)]
    pub fn is_terminated(&self) -> bool {
        self.state.relaxed() == TERMINATED
    }
}

/// Signal enum encapsulates both SyncSignal and AsyncSignal to enable them to operate in the same context
#[derive(Clone, Copy, Debug)]
pub enum Signal<T> {
    Sync(*const SyncSignal<T>),
    #[cfg(feature = "async")]
    Async(*const AsyncSignal<T>),
}
// Safety: if T is Send/Sync, the Signal<T> is safe to move
unsafe impl<T> Sync for Signal<T> {}
// Safety: if T is Send/Sync, the Signal<T> is safe to move
unsafe impl<T> Send for Signal<T> {}

#[allow(dead_code)]
impl<T> Signal<T> {
    /// Waits for the signal event in sync mode,
    /// Safety: it's only safe to wait for signals that are not terminated or finished
    pub unsafe fn wait(&self) -> bool {
        match self {
            Signal::Sync(sig) => (**sig).wait(),
            #[cfg(feature = "async")]
            Signal::Async(_sig) => unreachable!("async sig: sync wait must not happen"),
        }
    }

    /// Sends object to receive signal
    /// Safety: it's only safe to be called only once on the receive signals that are not terminated
    pub unsafe fn send(&self, d: T) {
        match self {
            Signal::Sync(sig) => SyncSignal::send(*sig, d),
            #[cfg(feature = "async")]
            Signal::Async(sig) => AsyncSignal::send(*sig, d),
        }
    }

    /// Receives object from send signal
    /// Safety: it's only safe to be called only once on send signals that are not terminated
    pub unsafe fn recv(&self) -> T {
        match self {
            Signal::Sync(sig) => SyncSignal::recv(*sig),
            #[cfg(feature = "async")]
            Signal::Async(sig) => AsyncSignal::recv(*sig),
        }
    }

    /// Terminates the signal and notifies its waiter
    /// Safety: it's only safe to be called only once on send/receive signals that are not finished or terminated
    pub unsafe fn terminate(&self) {
        match self {
            Signal::Sync(sig) => SyncSignal::terminate(*sig),
            #[cfg(feature = "async")]
            Signal::Async(sig) => AsyncSignal::terminate(*sig),
        }
    }

    /// Loads pointer data and drops it in place
    /// Safety: it should only be used once, and only when data in ptr is valid and not moved.
    pub unsafe fn load_and_drop(&self) {
        match self {
            Signal::Sync(sig) => SyncSignal::load_and_drop(*sig),
            #[cfg(feature = "async")]
            Signal::Async(sig) => AsyncSignal::load_and_drop(*sig),
        }
    }
}

impl<T> Default for Signal<T> {
    fn default() -> Self {
        // Safety: it's not safe to use this signal, it's only a place holder.
        Signal::Sync(std::ptr::null() as *const SyncSignal<T>)
    }
}

impl<T> PartialEq for Signal<T> {
    /// Returns whether the signal pointer is the same as other
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Sync(l0), Self::Sync(r0)) => l0 == r0,
            #[cfg(feature = "async")]
            (Self::Async(l0), Self::Async(r0)) => l0 == r0,
            #[cfg(feature = "async")]
            _ => false,
        }
    }
}
