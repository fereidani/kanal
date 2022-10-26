use crate::state::{State, LOCKED, LOCKED_STARVATION, TERMINATED, UNLOCKED};
use std::marker::PhantomData;
use std::task::Waker;
use std::thread::Thread;
use std::time::{Duration, Instant};

#[cfg(feature = "async")]
pub struct AsyncSignal<T> {
    state: State,
    data: *mut T,
    waker: Option<Waker>,
    phantum: PhantomData<Box<T>>,
}

#[cfg(feature = "async")]
unsafe impl<T> Send for AsyncSignal<T> {}

#[cfg(feature = "async")]
impl<T> std::future::Future for AsyncSignal<T> {
    type Output = u8;
    #[inline(always)]
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let v = self.state.value();
        if v < LOCKED {
            return std::task::Poll::Ready(v);
        }
        {
            self.waker = Some(cx.waker().clone())
        }
        let v = self.state.value();
        if v >= LOCKED {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(v)
        }
    }
}

/// read the pointer value for types with size bigger than zero, in case of zero sized types returns std::mem::zeroed
#[inline(always)]
unsafe fn read_ptr<T>(ptr: *const T) -> T {
    if std::mem::size_of::<T>() > 0 {
        std::ptr::read(ptr)
    } else {
        // for zero types
        std::mem::zeroed()
    }
}

#[cfg(feature = "async")]
impl<T> AsyncSignal<T> {
    // signal to send data to a writer
    #[inline(always)]
    pub fn new() -> Self {
        let e = Self {
            state: Default::default(),
            data: std::ptr::null_mut(),
            waker: Default::default(),
            phantum: PhantomData,
        };
        e.state.store(LOCKED);
        e
    }

    #[inline(always)]
    pub fn set_ptr(&mut self, data: *mut T) {
        self.data = data;
    }

    // convert async signal to common signal that works with channel internal
    #[inline(always)]
    pub fn as_signal(&self) -> Signal<T> {
        Signal::Async(self as *const Self)
    }

    #[inline(always)]
    pub unsafe fn send(this: *const Self, d: T) {
        if std::mem::size_of::<T>() > 0 {
            *(*this).data = d;
        }
        let waker = (*this).waker.as_ref().unwrap().clone();
        (*this).state.store(UNLOCKED);
        waker.wake();
    }

    #[inline(always)]
    pub unsafe fn recv(this: *const Self) -> T {
        if std::mem::size_of::<T>() > 0 {
            let waker = (*this).waker.as_ref().unwrap().clone();
            let r = read_ptr((*this).data);
            (*this).state.store(UNLOCKED);
            waker.wake();
            r
        } else {
            let waker = (*this).waker.as_ref().unwrap().clone();
            (*this).state.store(UNLOCKED);
            waker.wake();
            std::mem::zeroed()
        }
    }

    #[inline(always)]
    pub fn register(&mut self, waker: &Waker) {
        self.waker = Some(waker.clone())
    }

    // terminates operation and notifies the waiter , shall not be called more than once
    #[inline(always)]
    pub unsafe fn terminate(this: *const Self) {
        let waker = (*this).waker.as_ref().unwrap().clone();
        (*this).state.store(TERMINATED);
        waker.wake();
    }

    // waits for signal and returns true if send/recv operation was successful
    pub fn wait_indefinitely(&self) -> u8 {
        self.state.wait_indefinitely()
    }
}

pub struct SyncSignal<T> {
    state: State,
    ptr: *mut T,
    thread: Thread,
    phantum: PhantomData<Box<T>>,
}

unsafe impl<T> Send for SyncSignal<T> {}
unsafe impl<T> Sync for SyncSignal<T> {}

#[allow(dead_code)]
impl<T> SyncSignal<T> {
    #[inline(always)]
    pub fn new(ptr: *mut T, thread: Thread) -> Self {
        let e = SyncSignal {
            state: Default::default(),
            ptr,
            thread,
            phantum: PhantomData,
        };

        e.state.lock();
        e
    }

    // convert sync signal to common signal that works with channel internal
    pub fn as_signal(&self) -> Signal<T> {
        Signal::Sync(self as *const Self)
    }

    // writes data to pointer, shall not be called more than once
    // has to be done through a pointer because by the end of this scope
    // the object might have been destroyes by the owning receiver
    #[inline(always)]
    pub unsafe fn send(this: *const Self, d: T) {
        if std::mem::size_of::<T>() > 0 {
            std::ptr::write((*this).ptr, d);
        }
        if !(*this).state.unlock() {
            // Clone the thread because this.thread might be destroyed
            // sometime during unpark when the other thread wakes up
            let thread = (*this).thread.clone();
            (*this).state.force_unlock();
            thread.unpark();
        }
    }

    // read data from pointer, shall not be called more than once
    // has to be done through a pointer because by the end of this scope
    // the object might have been destroyed by the owning receiver
    #[inline(always)]
    pub unsafe fn recv(this: *const Self) -> T {
        let d = read_ptr((*this).ptr);
        if !(*this).state.unlock() {
            // Same as send
            let thread = (*this).thread.clone();
            (*this).state.force_unlock();
            thread.unpark();
        }
        d
    }

    // terminates operation and notifies the waiter , shall not be called more than once
    // has to be done through a pointer because by the end of this scope
    // the object might have been destroyed by the owner
    #[inline(always)]
    pub unsafe fn terminate(this: *const Self) {
        if !(*this).state.terminate() {
            // Same as send and recv
            let thread = (*this).thread.clone();
            (*this).state.force_terminate();
            thread.unpark();
        }
    }

    // waits for signal and returns true if send/recv operation was successful
    #[inline(always)]
    pub fn wait(&self) -> bool {
        // WAIT FOR UNLOCK
        let until = Instant::now() + Duration::from_nanos(1 << 18); //about 0.26ms
        let mut v = self.state.wait_unlock_until(until);

        if v < LOCKED {
            return v == UNLOCKED;
        }
        // enter starvation mod
        if self.state.upgrade_lock() {
            v = LOCKED_STARVATION;
            while v == LOCKED_STARVATION {
                std::thread::park();
                v = self.state.value();
            }
        } else {
            v = self.state.value();
        }
        v == UNLOCKED
    }

    // waits for signal and returns true if send/recv operation was successful
    #[inline(always)]
    pub fn wait_timeout(&self, until: Instant) -> bool {
        let v = self.state.wait_unlock_until(until);
        v == UNLOCKED
    }

    pub fn is_locked(&self) -> bool {
        self.state.value() >= LOCKED
    }

    #[inline(always)]
    pub fn is_terminated(&self) -> bool {
        self.state.value() == TERMINATED
    }
}

#[derive(Clone, Copy)]
pub enum Signal<T> {
    Sync(*const SyncSignal<T>),
    #[cfg(feature = "async")]
    Async(*const AsyncSignal<T>),
}

unsafe impl<T> Sync for Signal<T> {}
unsafe impl<T> Send for Signal<T> {}

#[allow(dead_code)]
impl<T> Signal<T> {
    pub unsafe fn wait(&self) -> bool {
        match self {
            Signal::Sync(sig) => (**sig).wait(),
            #[cfg(feature = "async")]
            Signal::Async(_sig) => panic!("async sig: sync wait must not happen"),
        }
    }

    pub unsafe fn send(self, d: T) {
        match self {
            Signal::Sync(sig) => SyncSignal::send(sig, d),
            #[cfg(feature = "async")]
            Signal::Async(sig) => AsyncSignal::send(sig, d),
        }
    }

    pub unsafe fn recv(self) -> T {
        match self {
            Signal::Sync(sig) => SyncSignal::recv(sig),
            #[cfg(feature = "async")]
            Signal::Async(sig) => AsyncSignal::recv(sig),
        }
    }

    pub unsafe fn terminate(&self) {
        match self {
            Signal::Sync(sig) => SyncSignal::terminate(*sig),
            #[cfg(feature = "async")]
            Signal::Async(sig) => AsyncSignal::terminate(*sig),
        }
    }
}

impl<T> PartialEq for Signal<T> {
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
