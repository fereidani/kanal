#[cfg(feature = "async")]
use crate::state::UNLOCKED;
#[cfg(feature = "async")]
use crate::FutureState;
use crate::{pointer::KanalPtr, signal::*};
#[cfg(feature = "async")]
use futures_core::Future;
use std::{
    fmt::Debug,
    marker::PhantomData,
    mem::{forget, size_of, MaybeUninit},
    sync::atomic::{AtomicUsize, Ordering},
};
#[cfg(feature = "async")]
use std::{
    mem::{needs_drop, transmute},
    task::Poll,
};

const WAITING: *const () = std::ptr::null_mut::<()>();
const FINISHED: *const () = !0usize as *const ();

enum ActionResult<T> {
    Ok,
    Racing,
    Winner(*const Signal<T>),
    Finish,
}

impl<T> ActionResult<T> {
    fn is_ok(&self) -> bool {
        matches!(self, ActionResult::Ok)
    }
}

impl<T> From<Result<usize, usize>> for ActionResult<T> {
    #[inline(always)]
    fn from(value: Result<usize, usize>) -> Self {
        match value {
            Ok(_) => ActionResult::Ok,
            Err(ptr) => {
                if ptr == WAITING as usize {
                    ActionResult::Racing
                } else if ptr == FINISHED as usize {
                    ActionResult::Finish
                } else {
                    ActionResult::Winner(ptr as *const Signal<T>)
                }
            }
        }
    }
}

struct OneshotInternal<T> {
    ptr: AtomicUsize,
    _phantom: PhantomData<Option<T>>,
}

struct OneshotInternalPointer<T> {
    ptr: *mut OneshotInternal<T>,
    _phantom: PhantomData<Option<T>>,
}

impl<T> Clone for OneshotInternalPointer<T> {
    fn clone(&self) -> Self {
        Self {
            ptr: self.ptr,
            _phantom: PhantomData,
        }
    }
}
impl<T> Copy for OneshotInternalPointer<T> {}

impl<T> OneshotInternalPointer<T> {
    unsafe fn as_ref(&self) -> &OneshotInternal<T> {
        &*self.ptr
    }
    unsafe fn drop(&self) {
        let _ = Box::from_raw(self.ptr);
    }
}
unsafe impl<T> Send for OneshotInternalPointer<T> {}

impl<T> OneshotInternal<T> {
    // Tries to inform other side that signal is ready.
    // If it's done successfully it means that performer will wait for otherside
    //  to awake its signal about task finishing state which is either finished or terminated.
    #[inline(always)]
    fn try_win_race(&self, own_ptr: *const Signal<T>) -> ActionResult<T> {
        ActionResult::from(self.ptr.compare_exchange(
            WAITING as usize,
            own_ptr as usize,
            Ordering::AcqRel,
            Ordering::Acquire,
        ))
    }
    // Tries to cancel the operation for both sides
    // It will update the state from racing with no winner to finishing
    // Which will be interpreted as termination of channel for other side
    #[inline(always)]
    fn try_terminate(&self) -> ActionResult<T> {
        ActionResult::from(self.ptr.compare_exchange(
            WAITING as usize,
            FINISHED as usize,
            Ordering::AcqRel,
            Ordering::Acquire,
        ))
    }
    // It's only used when the owner of signal in async context wants to update its waker
    //  as signal is shared in that context, it's not safe to write to the signal as there will be multiple
    //  mutable reference to signal, with this method channel resets state to racing if its possible and tries to
    //  win the race again after updating the signal, the logic is something similiar to the Mutex.
    #[inline(always)]
    #[cfg(feature = "async")]
    fn try_reset(&self, own_ptr: *const Signal<T>) -> ActionResult<T> {
        ActionResult::from(self.ptr.compare_exchange(
            own_ptr as usize,
            WAITING as usize,
            Ordering::AcqRel,
            Ordering::Acquire,
        ))
    }
    // Tries to upgrade state from waiting to finish
    // If winer use it, it's a signal for termination of signal
    // If loser use it, it's a signal for trying to start the data movement
    #[inline(always)]
    fn try_finish(&self, from: *const Signal<T>) -> ActionResult<T> {
        ActionResult::from(self.ptr.compare_exchange(
            from as usize,
            FINISHED as usize,
            Ordering::AcqRel,
            Ordering::Acquire,
        ))
    }
}

// Returns true if transfer was successfull
#[inline(never)]
fn try_drop_send_internal<T>(internal_ptr: OneshotInternalPointer<T>) -> bool {
    let internal = unsafe { internal_ptr.as_ref() };
    loop {
        // Safety: other side can't drop internal, if this side is not in finishing stage
        match internal.try_terminate() {
            ActionResult::Ok => {
                // Signal state is updated to finishing other side is responsible for dropping the internal
                return false;
            }
            ActionResult::Racing => unreachable!("already tried updating from this state"),
            ActionResult::Winner(receiver) => {
                if internal.try_finish(receiver).is_ok() {
                    // Safety: as status is waiting we know that we lost the race, so recv_signal is valid.
                    unsafe { SignalTerminator::from(receiver).terminate() };
                    return false;
                }
                // CONTINUE
            }
            ActionResult::Finish => {
                // Safety: other side updated status to finishing before, it's safe to release memory now.
                unsafe { internal_ptr.drop() }
                return true;
            }
        }
    }
}

// Returns true if transfer was successfull
#[inline(never)]
fn try_drop_recv_internal<T>(internal_ptr: OneshotInternalPointer<T>) -> bool {
    let internal = unsafe { internal_ptr.as_ref() };
    loop {
        // Safety: other side can't drop internal, if this side is not in finishing stage
        match internal.try_terminate() {
            ActionResult::Ok => {
                // Signal state is updated to finishing other side is responsible for dropping the internal
                return false;
            }
            ActionResult::Racing => unreachable!("already tried updating from this state"),
            ActionResult::Winner(sender) => {
                if internal.try_finish(sender).is_ok() {
                    // Safety: as status is waiting we know that we lost the race, so recv_signal is valid.
                    unsafe { SignalTerminator::from(sender).terminate() };
                    return false;
                }
                // CONTINUE
            }
            ActionResult::Finish => {
                // Safety: other side updated status to finishing before, it's safe to release memory now.
                unsafe { internal_ptr.drop() }
                return true;
            }
        }
    }
}
/// `OneshotReceiver<T>` is the sender side of oneshot channel that can send a single message
pub struct OneshotSender<T> {
    internal_ptr: OneshotInternalPointer<T>,
}

impl<T> Debug for OneshotSender<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OneshotSender {{ .. }}")
    }
}

impl<T> Drop for OneshotSender<T> {
    #[inline(always)]
    fn drop(&mut self) {
        try_drop_send_internal(self.internal_ptr);
    }
}

impl<T> OneshotSender<T> {
    /// Sends a message to the channel and consumes the send side
    #[inline(always)]
    pub fn send(self, data: T) -> Result<(), T> {
        let mut data = MaybeUninit::new(data);
        // Safety: other side can't drop internal, if this side is not in finishing stage
        let internal = unsafe { self.internal_ptr.as_ref() };
        let sig = Signal::new_sync(KanalPtr::new_from(data.as_mut_ptr()));
        loop {
            match internal.try_win_race(&sig) {
                ActionResult::Ok => {
                    let success = sig.wait();
                    unsafe {
                        // Safety: as convention specifies, winner or last leaving owner should drop the ptr
                        self.internal_ptr.drop();
                        forget(self);
                    }
                    if !success {
                        // Safety: data is inited for sure and is the only single copy of the object
                        return Err(unsafe { data.assume_init() });
                    }
                    return Ok(());
                }
                ActionResult::Racing => unreachable!("already tried updating from this state"),
                ActionResult::Winner(receiver) => {
                    match internal.try_finish(receiver) {
                        ActionResult::Ok => {
                            // Safety: recv_signal is guaranteed to be valid due to failed race situation
                            unsafe {
                                SignalTerminator::from(receiver).send_copy(data.as_ptr());
                            }
                            // Other side is responsible for dropping
                            forget(self);
                            return Ok(());
                        }
                        ActionResult::Racing => {
                            // Race is on again try to win the signal if it's possible
                            continue;
                        }
                        _ => unreachable!(),
                    }
                }
                ActionResult::Finish => {
                    unsafe {
                        // Safety: as convention specifies, winner or last leaving owner should drop the ptr
                        self.internal_ptr.drop();
                        forget(self);
                    }
                    // Safety: data is inited for sure and is the only single copy of the object
                    return Err(unsafe { data.assume_init() });
                }
            };
        }
    }
    /// Converts sender to async variant of it to be used in async context
    #[cfg(feature = "async")]
    pub fn to_async(self) -> OneshotAsyncSender<T> {
        // Safety: both structs are same with different methods
        unsafe { transmute(self) }
    }
}
/// `OneshotAsyncSender<T>` is the async sender side of the oneshot channel to send a single message in async context
#[cfg(feature = "async")]
pub struct OneshotAsyncSender<T> {
    internal_ptr: OneshotInternalPointer<T>,
}
#[cfg(feature = "async")]
impl<T> Debug for OneshotAsyncSender<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OneshotAsyncSender {{ .. }}")
    }
}
#[cfg(feature = "async")]
impl<T> Drop for OneshotAsyncSender<T> {
    fn drop(&mut self) {
        try_drop_send_internal(self.internal_ptr);
    }
}

/// `OneshotReceiver<T>` is the receiver side of oneshot channel that can receive a single message
pub struct OneshotReceiver<T> {
    internal_ptr: OneshotInternalPointer<T>,
}

impl<T> Debug for OneshotReceiver<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OneshotReceiver {{ .. }}")
    }
}

impl<T> Drop for OneshotReceiver<T> {
    fn drop(&mut self) {
        try_drop_recv_internal(self.internal_ptr);
    }
}

impl<T> OneshotReceiver<T> {
    /// Receives a message from the channel and consumes the receive side
    #[inline(always)]
    pub fn recv(self) -> Result<T, ()> {
        // Safety: other side can't drop internal, if this side is not in finishing stage
        let internal = unsafe { self.internal_ptr.as_ref() };
        let mut ret = MaybeUninit::<T>::uninit();
        let sig = Signal::new_sync(KanalPtr::new_write_address_ptr(ret.as_mut_ptr()));
        // Safety: self.internal is guaranteed to be valid through Arc reference
        loop {
            match internal.try_win_race(&sig) {
                ActionResult::Ok => {
                    // wait synchronously until data is received.
                    let success = sig.wait();
                    unsafe {
                        // Safety: as convention specifies, winner or last leaving owner should drop the ptr
                        self.internal_ptr.drop();
                        forget(self);
                    }
                    if !success {
                        return Err(());
                    }
                    // Safety: it's safe to assume init as data is forgotten on another side
                    return if size_of::<T>() > size_of::<*mut T>() {
                        Ok(unsafe { ret.assume_init() })
                    } else {
                        Ok(unsafe { sig.assume_init() })
                    };
                }
                ActionResult::Racing => unreachable!("already tried updating from this state"),
                ActionResult::Winner(sender) => {
                    match internal.try_finish(sender) {
                        ActionResult::Ok => {
                            // Other side is responsible for dropping
                            forget(self);
                            // Safety: signal is valid and recv will be called only once as this call will consume the self
                            return Ok(unsafe { SignalTerminator::from(sender).recv() });
                        }
                        ActionResult::Racing => {
                            // Race is on again try to win the signal if it's possible
                            continue;
                        }
                        ActionResult::Finish => {
                            unsafe {
                                // Safety: as convention specifies, winner or last leaving owner should drop the ptr
                                self.internal_ptr.drop();
                                forget(self);
                            }
                            return Err(());
                        }
                        _ => unreachable!(),
                    }
                }
                ActionResult::Finish => {
                    unsafe {
                        // Safety: as convention specifies, winner or last leaving owner should drop the ptr
                        self.internal_ptr.drop();
                        forget(self);
                    }
                    return Err(());
                }
            };
        }
    }

    /// Converts receiver to async variant of it to be used in async context
    #[cfg(feature = "async")]
    pub fn to_async(self) -> OneshotAsyncReceiver<T> {
        // Safety: both structs are same with different methods
        unsafe { transmute(self) }
    }
}

/// `OneshotAsyncReceiver<T>` is the async receiver side of the oneshot channel to receive a single message in async context
#[cfg(feature = "async")]
pub struct OneshotAsyncReceiver<T> {
    internal_ptr: OneshotInternalPointer<T>,
}
#[cfg(feature = "async")]
impl<T> Debug for OneshotAsyncReceiver<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OneshotAsyncReceiver {{ .. }}")
    }
}
#[cfg(feature = "async")]
impl<T> Drop for OneshotAsyncReceiver<T> {
    fn drop(&mut self) {
        try_drop_recv_internal(self.internal_ptr);
    }
}

#[cfg(feature = "async")]
impl<T> OneshotAsyncSender<T> {
    /// Returns a send future for sending `data` to the channel and consumes the send side
    pub fn send(self, data: T) -> OneshotSendFuture<T> {
        let internal_ptr = self.internal_ptr;
        // No need to worry about droping the internal pointer in self, future is the new owner of the pointer.
        forget(self);
        if size_of::<T>() > size_of::<*mut T>() {
            OneshotSendFuture {
                state: FutureState::Zero,
                internal_ptr,
                sig: Signal::new_async(),
                data: MaybeUninit::new(data),
            }
        } else {
            OneshotSendFuture {
                state: FutureState::Zero,
                internal_ptr,
                sig: Signal::new_async_ptr(KanalPtr::new_owned(data)),
                data: MaybeUninit::uninit(),
            }
        }
    }
    /// Converts async sender to sync variant of it to be used in sync context
    pub fn to_sync(self) -> OneshotSender<T> {
        // Safety: both structs are same with different methods
        unsafe { transmute(self) }
    }
}
#[cfg(feature = "async")]
impl<T> OneshotAsyncReceiver<T> {
    /// Returns a receive future for receiving data from the channel and consumes the receive side
    pub fn recv(self) -> OneshotReceiveFuture<T> {
        let internal_ptr = self.internal_ptr;
        // No need to worry about droping the internal pointer in self, future is the new owner of the pointer.
        forget(self);
        OneshotReceiveFuture {
            state: FutureState::Zero,
            internal_ptr,
            sig: Signal::new_async(),
            data: MaybeUninit::uninit(),
        }
    }
    /// Converts async receiver to sync variant of it to be used in sync context
    pub fn to_sync(self) -> OneshotReceiver<T> {
        // Safety: both structs are same with different methods
        unsafe { transmute(self) }
    }
}

/// Creates new oneshot channel and returns the sender and the receiver for it.
pub fn oneshot<T>() -> (OneshotSender<T>, OneshotReceiver<T>) {
    let ptr = Box::into_raw(Box::new(OneshotInternal {
        ptr: (WAITING as usize).into(),
        _phantom: PhantomData,
    }));
    (
        OneshotSender {
            internal_ptr: OneshotInternalPointer {
                ptr,
                _phantom: PhantomData,
            },
        },
        OneshotReceiver {
            internal_ptr: OneshotInternalPointer {
                ptr,
                _phantom: PhantomData,
            },
        },
    )
}

/// Creates new oneshot channel and returns the async sender and the async receiver for it.
#[cfg(feature = "async")]
pub fn oneshot_async<T>() -> (OneshotAsyncSender<T>, OneshotAsyncReceiver<T>) {
    let ptr = Box::into_raw(Box::new(OneshotInternal {
        ptr: (WAITING as usize).into(),
        _phantom: PhantomData,
    }));
    (
        OneshotAsyncSender {
            internal_ptr: OneshotInternalPointer {
                ptr,
                _phantom: PhantomData,
            },
        },
        OneshotAsyncReceiver {
            internal_ptr: OneshotInternalPointer {
                ptr,
                _phantom: PhantomData,
            },
        },
    )
}

/// Oneshot channel send future that asynchronously sends data to the oneshot receiver
#[must_use = "futures do nothing unless you .await or poll them"]
#[cfg(feature = "async")]
pub struct OneshotSendFuture<T> {
    state: FutureState,
    internal_ptr: OneshotInternalPointer<T>,
    sig: Signal<T>,
    data: MaybeUninit<T>,
}

#[cfg(feature = "async")]
impl<T> OneshotSendFuture<T> {
    /// # Safety
    /// it's only safe to call this function once and only if send operation will finish after this call.
    #[inline(always)]
    unsafe fn read_local_data(&self) -> T {
        if size_of::<T>() > size_of::<*mut T>() {
            // if its smaller than register size, it does not need pointer setup as data will be stored in register address object
            std::ptr::read(self.data.as_ptr())
        } else {
            self.sig.assume_init()
        }
    }
    /// # Safety
    /// it's only safe to call this function once and only if send operation fails
    #[inline(always)]
    unsafe fn drop_local_data(&mut self) {
        if size_of::<T>() > size_of::<*mut T>() {
            self.data.assume_init_drop();
        } else {
            self.sig.load_and_drop();
        }
    }
}
#[cfg(feature = "async")]
impl<T> Debug for OneshotSendFuture<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OneshotSendFuture {{ .. }}")
    }
}

/// Oneshot channel receive future that asynchronously receives data from oneshot sender
#[must_use = "futures do nothing unless you .await or poll them"]
#[cfg(feature = "async")]
pub struct OneshotReceiveFuture<T> {
    state: FutureState,
    internal_ptr: OneshotInternalPointer<T>,
    sig: Signal<T>,
    data: MaybeUninit<T>,
}

#[cfg(feature = "async")]
impl<T> Debug for OneshotReceiveFuture<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OneshotReceiveFuture {{ .. }}")
    }
}

#[cfg(feature = "async")]
impl<T> OneshotReceiveFuture<T> {
    /// # Safety
    /// it's only safe to call this function once and only if send operation will finish after this call.
    #[inline(always)]
    unsafe fn read_local_data(&self) -> T {
        if size_of::<T>() > size_of::<*mut T>() {
            // if its smaller than register size, it does not need pointer setup as data will be stored in register address object
            std::ptr::read(self.data.as_ptr())
        } else {
            self.sig.assume_init()
        }
    }
    /// # Safety
    /// it's only safe to call this function once and only if send operation fails
    #[inline(always)]
    unsafe fn drop_local_data(&mut self) {
        if size_of::<T>() > size_of::<*mut T>() {
            self.data.assume_init_drop();
        } else {
            self.sig.load_and_drop();
        }
    }
}

#[cfg(feature = "async")]
impl<T> Future for OneshotSendFuture<T> {
    type Output = Result<(), T>;
    #[inline(always)]
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        // Safety: we will never move data outside of this
        let this = unsafe { self.get_unchecked_mut() };
        // Safety: other side can't drop internal, if this side is not in finishing stage
        let internal = unsafe { this.internal_ptr.as_ref() };
        loop {
            match this.state {
                FutureState::Zero => {
                    if size_of::<T>() > size_of::<*mut T>() {
                        // if type T smaller than register size, it does not need pointer setup as data will be stored in register address object
                        this.sig
                            .set_ptr(KanalPtr::new_unchecked(this.data.as_mut_ptr()));
                    }
                    this.sig.register_waker(cx.waker());

                    match internal.try_win_race(&this.sig) {
                        ActionResult::Ok => {
                            this.state = FutureState::Waiting;
                            return Poll::Pending;
                        }
                        ActionResult::Racing => unreachable!(),
                        ActionResult::Winner(receiver) => match internal.try_finish(receiver) {
                            ActionResult::Ok => {
                                this.state = FutureState::Done;
                                // Other side is responsible for dropping
                                // Safety: signal is valid and recv will be called only once as this call will consume the self
                                return unsafe {
                                    Poll::Ready({
                                        SignalTerminator::from(receiver)
                                            .send(this.read_local_data());
                                        Ok(())
                                    })
                                };
                            }
                            ActionResult::Racing => {
                                // Race is on again retry wining the signal
                                continue;
                            }
                            ActionResult::Winner(_) => unreachable!(),
                            ActionResult::Finish => {
                                this.state = FutureState::Done;
                                // Safety: other side is dropped
                                unsafe {
                                    this.internal_ptr.drop();
                                }
                                return Poll::Ready(Err(unsafe { this.read_local_data() }));
                            }
                        },
                        ActionResult::Finish => {
                            this.state = FutureState::Done;
                            // Safety: other side is dropped
                            unsafe {
                                this.internal_ptr.drop();
                            }
                            return Poll::Ready(Err(unsafe { this.read_local_data() }));
                        }
                    }
                }
                FutureState::Waiting => {
                    let r = this.sig.poll();
                    match r {
                        Poll::Ready(v) => {
                            this.state = FutureState::Done;
                            // Safety: winner is this side and transfer is done, this side is responsible for dropping the internal
                            unsafe {
                                this.internal_ptr.drop();
                            }
                            if v == UNLOCKED {
                                // Safety: transfer was successfull and local data is valid to read
                                return Poll::Ready(Ok(()));
                            }
                            return Poll::Ready(Err(unsafe { this.read_local_data() }));
                        }
                        Poll::Pending => {
                            if !this.sig.will_wake(cx.waker()) {
                                // the Waker is changed and we need to update waker, but we can't because it's possible to simultaneously other side pick the signal
                                //    and receive mutable access to it, so we will try to reset state to racing, update waker, then trying to win the signal pointer again
                                if internal.try_reset(&this.sig).is_ok() {
                                    // Signal pointer is released, participate in the race again
                                    this.state = FutureState::Waiting;
                                    continue;
                                }
                                this.state = FutureState::Done;
                                // the signal is already shared, and data will be available shortly, so wait synchronously and return the result
                                // note: it's not possible safely to update waker after the signal is shared, but we know data will be ready shortly,
                                //   we can wait synchronously and receive it.
                                let success = this.sig.async_blocking_wait();
                                // Safety: winner is this side and transfer is done, this side is responsible for dropping the internal
                                unsafe {
                                    this.internal_ptr.drop();
                                }
                                if success {
                                    return Poll::Ready(Ok(()));
                                }
                                return Poll::Ready(Err(unsafe { this.read_local_data() }));
                            }
                            return Poll::Pending;
                        }
                    }
                }
                FutureState::Done => {
                    panic!("result has already been returned")
                }
            }
        }
    }
}

#[cfg(feature = "async")]
impl<T> Future for OneshotReceiveFuture<T> {
    type Output = Result<T, ()>;
    #[inline(always)]
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        // Safety: we will never move data outside of this
        let this = unsafe { self.get_unchecked_mut() };
        // Safety: other side can't drop internal, if this side is not in finishing stage
        let internal = unsafe { this.internal_ptr.as_ref() };
        loop {
            match this.state {
                FutureState::Zero => {
                    if size_of::<T>() > size_of::<*mut T>() {
                        // if type T smaller than register size, it does not need pointer setup as data will be stored in register address object
                        this.sig
                            .set_ptr(KanalPtr::new_unchecked(this.data.as_mut_ptr()));
                    }
                    this.sig.register_waker(cx.waker());

                    match internal.try_win_race(&this.sig) {
                        ActionResult::Ok => {
                            this.state = FutureState::Waiting;
                            return Poll::Pending;
                        }
                        ActionResult::Racing => unreachable!(),
                        ActionResult::Winner(sender) => match internal.try_finish(sender) {
                            ActionResult::Ok => {
                                this.state = FutureState::Done;
                                // Other side is responsible for dropping
                                // Safety: signal is valid and recv will be called only once as this call will consume the self
                                return unsafe {
                                    Poll::Ready(Ok(SignalTerminator::from(sender).recv()))
                                };
                            }
                            ActionResult::Racing => {
                                // Race is on again retry wining the signal
                                continue;
                            }
                            ActionResult::Winner(_) => unreachable!(),
                            ActionResult::Finish => {
                                this.state = FutureState::Done;
                                // Safety: other side is dropped
                                unsafe {
                                    this.internal_ptr.drop();
                                }
                                return Poll::Ready(Err(()));
                            }
                        },
                        ActionResult::Finish => {
                            this.state = FutureState::Done;
                            // Safety: other side is dropped
                            unsafe {
                                this.internal_ptr.drop();
                            }
                            return Poll::Ready(Err(()));
                        }
                    }
                }
                FutureState::Waiting => {
                    let r = this.sig.poll();
                    match r {
                        Poll::Ready(v) => {
                            this.state = FutureState::Done;
                            // Safety: winner is this side and transfer is done, this side is responsible for dropping the internal
                            unsafe {
                                this.internal_ptr.drop();
                            }
                            if v == UNLOCKED {
                                // Safety: transfer was successfull and local data is valid to read
                                return Poll::Ready(Ok(unsafe { this.read_local_data() }));
                            }
                            return Poll::Ready(Err(()));
                        }
                        Poll::Pending => {
                            if !this.sig.will_wake(cx.waker()) {
                                // the Waker is changed and we need to update waker, but we can't because it's possible to simultaneously other side pick the signal
                                //    and receive mutable access to it, so we will try to reset state to racing, update waker, then trying to win the signal pointer again
                                if internal.try_reset(&this.sig).is_ok() {
                                    // Signal pointer is released, participate in the race again
                                    this.state = FutureState::Waiting;
                                    continue;
                                }
                                this.state = FutureState::Done;
                                // the signal is already shared, and data will be available shortly, so wait synchronously and return the result
                                // note: it's not possible safely to update waker after the signal is shared, but we know data will be ready shortly,
                                //   we can wait synchronously and receive it.
                                let success = this.sig.async_blocking_wait();
                                // Safety: winner is this side and transfer is done, this side is responsible for dropping the internal
                                unsafe {
                                    this.internal_ptr.drop();
                                }
                                if success {
                                    return Poll::Ready(Ok(unsafe { this.read_local_data() }));
                                }
                                return Poll::Ready(Err(()));
                            }

                            return Poll::Pending;
                        }
                    }
                }
                FutureState::Done => {
                    panic!("result has already been returned")
                }
            }
        }
    }
}

#[cfg(feature = "async")]
impl<T> Drop for OneshotSendFuture<T> {
    #[inline(always)]
    fn drop(&mut self) {
        match self.state {
            FutureState::Zero => {
                try_drop_send_internal(self.internal_ptr);
            }
            FutureState::Waiting => {
                // If local state is waiting, this side won the signal pointer, loser will never go to waiting state under no condition.
                // Safety: other side can't drop internal, if this side is not in finishing stage
                let internal = unsafe { self.internal_ptr.as_ref() };
                match internal.try_finish(&self.sig) {
                    ActionResult::Ok => {
                        // Otherside is responsible for dropping internal
                        // This future can't return the owned send data as error so it should drop it
                        if needs_drop::<T>() {
                            unsafe {
                                self.drop_local_data();
                            }
                        }
                    }
                    ActionResult::Racing | ActionResult::Winner(_) => unreachable!(),
                    ActionResult::Finish => {
                        // otherside already received signal wait for the result
                        if !self.sig.async_blocking_wait() {
                            // This future can't return the owned send data as error so it should drop it
                            if needs_drop::<T>() {
                                unsafe {
                                    self.drop_local_data();
                                }
                            }
                        }
                        // Safety: winner is this side and transfer is done, this side is responsible for dropping the internal
                        unsafe {
                            self.internal_ptr.drop();
                        }
                    }
                }
            }
            FutureState::Done => {
                // No action required, when future reaches .join().unwrap();the done state, it already handled the internal_ptr drop in its future execution
            }
        }
    }
}

#[cfg(feature = "async")]
impl<T> Drop for OneshotReceiveFuture<T> {
    #[inline(always)]
    fn drop(&mut self) {
        match self.state {
            FutureState::Zero => {
                try_drop_recv_internal(self.internal_ptr);
            }
            FutureState::Waiting => {
                // If local state is waiting, this side won the signal pointer, loser will never go to waiting state under no condition.
                // Safety: other side can't drop internal, if this side is not in finishing stage
                let internal = unsafe { self.internal_ptr.as_ref() };
                match internal.try_finish(&self.sig) {
                    ActionResult::Ok => {
                        // Otherside is responsible for dropping internal
                    }
                    ActionResult::Racing | ActionResult::Winner(_) => unreachable!(),
                    ActionResult::Finish => {
                        // otherside already received signal wait for the result
                        if self.sig.async_blocking_wait() {
                            // to avoid memory leaks receiver should drop the local data as it can't return it
                            if needs_drop::<T>() {
                                unsafe {
                                    self.drop_local_data();
                                }
                            }
                        }
                        // Safety: winner is this side and transfer is done, this side is responsible for dropping the internal
                        unsafe {
                            self.internal_ptr.drop();
                        }
                    }
                }
            }
            FutureState::Done => {
                // No action required, when future reaches the done state, it already handled the internal_ptr drop in its future execution
            }
        }
    }
}
