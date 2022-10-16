use std::cell::Cell;

use std::future::Future;
use std::mem::ManuallyDrop;
use std::ptr::null_mut;

use std::task::Waker;
use std::thread::Thread;
use std::time::{Duration, SystemTime};

use crate::mutex::Mutex;
use crate::state::{State, LOCKED, LOCKED_STARVATION, TERMINATED, UNLOCKED};

#[cfg(feature = "async")]
pub struct AsyncSignal<T> {
    // async_lock: AtomicU8,
    data: *mut ManuallyDrop<T>,
    state: State,
    waker: WakerStore,
}

#[cfg(feature = "async")]
impl<T> Default for AsyncSignal<T> {
    fn default() -> Self {
        Self {
            data: null_mut(),
            state: Default::default(),
            waker: Default::default(),
        }
    }
}

#[cfg(feature = "async")]
unsafe impl<T> Send for AsyncSignal<T> {}

#[derive(Default)]
pub struct WakerStore(Mutex<Cell<Option<Waker>>>);

impl WakerStore {
    pub fn register(&self, w: &Waker) {
        let waker = self.0.lock();
        waker.set(Some(w.clone()));
    }
    pub fn wake(&self) {
        let waker = self.0.lock();
        if let Some(w) = waker.take() {
            w.wake();
        }
    }
}

impl<T> Future for AsyncSignal<T> {
    type Output = u8;
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let v = self.state.value();
        if v < LOCKED {
            return std::task::Poll::Ready(v);
        }
        {
            self.waker.register(cx.waker())
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

impl<T> AsyncSignal<T> {
    // signal to send data to a writer
    #[inline(always)]
    pub fn new() -> Self {
        let e = Self {
            state: Default::default(),
            data: null_mut(),
            waker: Default::default(),
        };
        e.state.store(LOCKED);
        e
    }

    #[inline(always)]
    pub fn set_ptr(&mut self, data: &mut ManuallyDrop<T>) {
        self.data = data as *mut ManuallyDrop<T>;
    }

    // convert async signal to common signal that works with channel internal
    #[inline(always)]
    pub fn as_signal(&mut self) -> Signal<T> {
        Signal::Async(self as *const Self)
    }

    // signals waiter that its request is processed
    #[inline(always)]
    fn wake(&self) {
        self.waker.wake();
    }

    #[inline(always)]
    pub unsafe fn send(&self, d: T) {
        if std::mem::size_of::<T>() > 0 {
            *self.data = ManuallyDrop::new(d);
        }
        self.state.store(UNLOCKED);
        self.wake();
    }

    #[inline(always)]
    pub unsafe fn recv(&self) -> T {
        if std::mem::size_of::<T>() > 0 {
            let r = ManuallyDrop::take(&mut *self.data);
            self.state.store(UNLOCKED);
            self.wake();
            r
        } else {
            self.state.store(UNLOCKED);
            self.wake();
            std::mem::zeroed()
        }
    }

    // terminates operation and notifies the waiter , shall not be called more than once
    #[inline(always)]
    pub unsafe fn terminate(&self) {
        self.state.store(TERMINATED);
        self.wake();
    }

    // wait for short time for lock and returns true if lock is unlocked
    #[inline(always)]
    pub fn wait_sync_short(&self) -> u8 {
        let until = SystemTime::now() + Duration::from_nanos(1e6 as u64);
        self.state.wait_unlock_until(until)
    }

    // waits for signal and returns true if send/recv operation was successful
    #[inline(always)]
    pub fn wait_indefinitely(&self) -> u8 {
        self.state.wait_indefinitely()
    }
}

pub struct SyncSignal<T> {
    ptr: *mut T,
    state: State,
    thread: Thread,
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
        //let mut v = self.state.wait_unlock_some();
        let until = SystemTime::now() + Duration::from_millis(1);
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
    pub fn wait_timeout(&self, until: SystemTime) -> bool {
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
    Async(*const AsyncSignal<T>),
}

unsafe impl<T> Sync for Signal<T> {}
unsafe impl<T> Send for Signal<T> {}

#[allow(dead_code)]
impl<T> Signal<T> {
    pub unsafe fn wait(&self) -> bool {
        match self {
            Signal::Sync(sig) => (**sig).wait(),
            Signal::Async(_sig) => panic!("async sig: sync wait must not happen"),
        }
    }

    pub unsafe fn wait_short(&self) -> u8 {
        match self {
            Signal::Sync(_sig) => panic!("sync sig: wait short must not happen"),
            Signal::Async(sig) => (**sig).wait_sync_short(),
        }
    }

    pub unsafe fn send(&self, d: T) {
        match self {
            Signal::Sync(sig) => SyncSignal::send(*sig, d),
            Signal::Async(sig) => (**sig).send(d),
        }
    }

    pub unsafe fn recv(&mut self) -> T {
        match self {
            Signal::Sync(sig) => SyncSignal::recv(*sig),
            Signal::Async(sig) => (**sig).recv(),
        }
    }

    pub unsafe fn terminate(&self) {
        match self {
            Signal::Sync(sig) => SyncSignal::terminate(*sig),
            Signal::Async(sig) => (**sig).terminate(),
        }
    }
}

impl<T> PartialEq for Signal<T> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Sync(l0), Self::Sync(r0)) => l0 == r0,
            (Self::Async(l0), Self::Async(r0)) => l0 == r0,
            _ => false,
        }
    }
}
