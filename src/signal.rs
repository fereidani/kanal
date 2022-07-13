use std::cell::Cell;

use std::hint::spin_loop;
use std::ops::Deref;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;

use std::future::Future;
use std::task::Waker;
use std::time::{Duration, SystemTime};

use crate::mutex::Mutex;
use crate::state::State;

pub struct AsyncSignal<T> {
    // async_lock: AtomicU8,
    ptr: AtomicPtr<T>,
    state: State,
    waker: WakerStore,
}

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

impl<T> Future for &AsyncSignal<T> {
    type Output = u8;
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let v = self.state.value();
        if v != 1 {
            return std::task::Poll::Ready(v);
        }
        {
            self.waker.register(cx.waker())
        }
        let v = self.state.value();
        if v == 1 {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(v)
        }
    }
}

impl<T> AsyncSignal<T> {
    #[inline(always)]
    pub fn new(ptr: AtomicPtr<T>) -> Arc<Self> {
        let e = Self {
            state: Default::default(),
            ptr,
            waker: Default::default(),
        };

        e.state.store(1);

        e.into()
    }

    // convert async signal to common signal that works with channel internal
    pub fn as_signal(self: Arc<Self>) -> Signal<T> {
        Signal::Async(self)
    }

    // signals waiter that its request is processed
    #[inline(always)]
    fn wake(&self) {
        self.waker.wake();
    }

    // writes data to pointer, shall not be called more than once
    #[inline(always)]
    pub unsafe fn send(&self, d: T) {
        std::ptr::write(self.ptr.load(Ordering::SeqCst), d);
        self.state.store(0);
        self.wake();
    }

    // wait for short time for lock and returns true if lock is unlocked
    #[inline(always)]
    pub fn wait_sync(&self) -> u8 {
        let until = SystemTime::now() + Duration::from_micros(1);
        while self.state.is_locked() && SystemTime::now() < until {
            spin_loop()
        }
        self.state.value()
    }

    // read data from pointer, shall not be called more than once
    #[inline(always)]
    pub unsafe fn recv(&self) -> T {
        let d = std::ptr::read(self.ptr.load(Ordering::SeqCst));
        self.state.store(0);
        self.wake();
        d
    }

    // terminates operation and notifies the waiter , shall not be called more than once
    #[inline(always)]
    pub unsafe fn terminate(&self) {
        self.state.store(2);
        self.wake();
    }
}

pub struct SyncSignal<T> {
    ptr: *mut T,
    s: State,
}

unsafe impl<T> Send for SyncSignal<T> {}
unsafe impl<T> Sync for SyncSignal<T> {}

impl<T> SyncSignal<T> {
    #[inline(always)]
    pub fn new(ptr: *mut T) -> Self {
        let e = SyncSignal {
            s: Default::default(),
            ptr,
        };

        e.s.lock();
        e
    }

    // convert sync signal to common signal that works with channel internal
    pub fn as_signal(&mut self) -> Signal<T> {
        Signal::Sync(self as *mut Self)
    }

    // writes data to pointer, shall not be called more than once
    #[inline(always)]
    pub unsafe fn send(&self, d: T) {
        if std::mem::size_of::<T>() > 0 {
            std::ptr::write(self.ptr, d);
        }
        self.s.unlock();
    }

    // read data from pointer, shall not be called more than once
    #[inline(always)]
    pub unsafe fn recv(&self) -> T {
        let d = std::ptr::read(self.ptr);
        self.s.unlock();
        d
    }

    // terminates operation and notifies the waiter , shall not be called more than once
    #[inline(always)]
    pub unsafe fn terminate(&self) {
        self.s.terminate();
    }

    // wait's for lock to get freed, returns status code of event, 1 done, 2 channel closed
    #[inline(always)]
    pub fn wait(&self) -> bool {
        // WAIT FOR UNLOCK
        self.s.wait_unlock() == 0
    }
}

pub enum Signal<T> {
    Sync(*const SyncSignal<T>),
    Async(Arc<AsyncSignal<T>>),
}

unsafe impl<T> Sync for Signal<T> {}
unsafe impl<T> Send for Signal<T> {}

impl<T> Signal<T> {
    pub unsafe fn wait(&self) -> bool {
        match self {
            Signal::Sync(sig) => (&**sig as &SyncSignal<T>).wait(),
            Signal::Async(_sig) => panic!("async sig: sync wait shall not happened"),
        }
    }

    pub async unsafe fn wait_async(self) -> bool {
        match self {
            Signal::Sync(_sig) => panic!("sync sig: async await shall not happened"),
            Signal::Async(sig) => sig.deref().await == 0,
        }
    }

    pub unsafe fn send(&self, d: T) {
        match self {
            Signal::Sync(sig) => (&**sig as &SyncSignal<T>).send(d),
            Signal::Async(sig) => sig.send(d),
        }
    }

    pub unsafe fn recv(&self) -> T {
        match self {
            Signal::Sync(sig) => (&**sig as &SyncSignal<T>).recv(),
            Signal::Async(sig) => sig.recv(),
        }
    }

    pub unsafe fn terminate(&self) {
        match self {
            Signal::Sync(sig) => (&**sig as &SyncSignal<T>).terminate(),
            Signal::Async(sig) => sig.terminate(),
        }
    }
}
