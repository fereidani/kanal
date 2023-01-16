use crate::{
    internal::{acquire_internal, Internal},
    pointer::KanalPtr,
    signal::Signal,
    AsyncReceiver, ReceiveError, SendError,
};
use futures_core::{FusedStream, Future, Stream};
use std::{
    fmt::Debug,
    marker::PhantomPinned,
    mem::{needs_drop, size_of, MaybeUninit},
    pin::Pin,
    task::Poll,
};

#[repr(u8)]
#[derive(PartialEq, Clone, Copy)]
pub(crate) enum FutureState {
    Zero,
    Waiting,
    Done,
}

impl FutureState {
    #[inline(always)]
    fn is_waiting(&self) -> bool {
        *self == FutureState::Waiting
    }
    #[inline(always)]
    fn is_done(&self) -> bool {
        *self == FutureState::Done
    }
}

/// Send future to send an object to the channel asynchronously
/// It must be polled to perform send action
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T> {
    pub(crate) state: FutureState,
    pub(crate) internal: &'a Internal<T>,
    pub(crate) sig: Signal<T>,
    pub(crate) data: MaybeUninit<T>,
    pub(crate) _pinned: PhantomPinned,
}

impl<'a, T> Debug for SendFuture<'a, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SendFuture {{ .. }}")
    }
}

impl<'a, T> Drop for SendFuture<'a, T> {
    fn drop(&mut self) {
        if !self.state.is_done() {
            if self.state.is_waiting()
                && !acquire_internal(self.internal).cancel_send_signal(&self.sig)
            {
                // a receiver got signal ownership, should wait until the response
                if self.sig.async_blocking_wait() {
                    // no need to drop data is moved to receiver
                    return;
                }
            }
            // signal is canceled, or in zero stated, drop data locally
            if needs_drop::<T>() {
                // Safety: data is not moved it's safe to drop it
                unsafe {
                    self.drop_local_data();
                }
            }
        }
    }
}

impl<'a, T> SendFuture<'a, T> {
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

impl<'a, T> Future for SendFuture<'a, T> {
    type Output = Result<(), SendError>;

    #[inline(always)]
    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        match this.state {
            FutureState::Zero => {
                let mut internal = acquire_internal(this.internal);
                if internal.recv_count == 0 {
                    let send_count = internal.send_count;
                    drop(internal);
                    this.state = FutureState::Done;
                    if needs_drop::<T>() {
                        // the data failed to move, drop it locally
                        // Safety: the data is not moved, we are sure that it is inited in this point, it's safe to init drop it.
                        unsafe {
                            this.drop_local_data();
                        }
                    }
                    if send_count == 0 {
                        return Poll::Ready(Err(SendError::Closed));
                    }
                    return Poll::Ready(Err(SendError::ReceiveClosed));
                }
                if let Some(first) = internal.next_recv() {
                    drop(internal);
                    this.state = FutureState::Done;
                    // Safety: data is inited and available from constructor
                    unsafe { first.send(this.read_local_data()) }
                    Poll::Ready(Ok(()))
                } else if internal.queue.len() < internal.capacity {
                    this.state = FutureState::Done;
                    // Safety: data is inited and available from constructor
                    internal.queue.push_back(unsafe { this.read_local_data() });
                    drop(internal);
                    Poll::Ready(Ok(()))
                } else {
                    this.state = FutureState::Waiting;
                    // if T is smaller than register size, we already have data in pointer address from initialization step
                    if size_of::<T>() > size_of::<*mut T>() {
                        this.sig
                            .set_ptr(KanalPtr::new_unchecked(this.data.as_mut_ptr()));
                    }
                    this.sig.register_waker(cx.waker());
                    // send directly to the waitlist
                    internal.push_send(this.sig.get_terminator());
                    drop(internal);
                    Poll::Pending
                }
            }
            FutureState::Waiting => {
                // waker is same no need to update
                // Safety: this.sig is pinned, sig is pinned too
                let r = this.sig.poll();
                match r {
                    Poll::Ready(success) => {
                        this.state = FutureState::Done;
                        if success {
                            Poll::Ready(Ok(()))
                        } else {
                            if needs_drop::<T>() {
                                // the data failed to move, drop it locally
                                // Safety: the data is not moved, we are sure that it is inited in this point, it's safe to init drop it.
                                unsafe { this.drop_local_data() };
                            }
                            Poll::Ready(Err(SendError::Closed))
                        }
                    }
                    Poll::Pending => {
                        if !this.sig.will_wake(cx.waker()) {
                            // Waker is changed and we need to update waker in the waiting list
                            {
                                let internal = acquire_internal(this.internal);
                                if internal.send_signal_exists(&this.sig) {
                                    // signal is not shared with other thread yet so it's safe to update waker locally
                                    this.sig.register_waker(cx.waker());
                                    return Poll::Pending;
                                }
                            }
                            // signal is already shared, and data will be available shortly, so wait synchronously and return the result
                            // note: it's not possible safely to update waker after the signal is shared, but we know data will be ready shortly,
                            //   we can wait synchronously and receive it.
                            this.state = FutureState::Done;
                            if this.sig.async_blocking_wait() {
                                return Poll::Ready(Ok(()));
                            }
                            // the data failed to move, drop it locally
                            // Safety: the data is not moved, we are sure that it is inited in this point, it's safe to init drop it.
                            if needs_drop::<T>() {
                                unsafe {
                                    this.drop_local_data();
                                }
                            }
                            return Poll::Ready(Err(SendError::Closed));
                        }
                        Poll::Pending
                    }
                }
            }
            _ => {
                panic!("polled after result is already returned")
            }
        }
    }
}

/// Receive future to receive an object from the channel asynchronously
/// It must be polled to perform receive action
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct ReceiveFuture<'a, T> {
    state: FutureState,
    internal: &'a Internal<T>,
    sig: Signal<T>,
    data: MaybeUninit<T>,
    is_stream: bool,
    _pinned: PhantomPinned,
}
impl<'a, T> Drop for ReceiveFuture<'a, T> {
    fn drop(&mut self) {
        if self.state.is_waiting() {
            // try to cancel recv signal
            if !acquire_internal(self.internal).cancel_recv_signal(&self.sig) {
                // a sender got signal ownership, receiver should wait until the response
                if self.sig.async_blocking_wait() {
                    // got ownership of data that is not going to be used ever again, so drop it
                    if needs_drop::<T>() {
                        // Safety: data is not moved it's safe to drop it
                        unsafe {
                            self.drop_local_data();
                        }
                    }
                }
            }
        }
    }
}

impl<'a, T> Debug for ReceiveFuture<'a, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ReceiveFuture {{ .. }}")
    }
}

impl<'a, T> ReceiveFuture<'a, T> {
    #[inline(always)]
    unsafe fn read_local_data(&self) -> T {
        if size_of::<T>() > size_of::<*mut T>() {
            // if T is smaller than register size, it does not need pointer setup as data will be stored in register address object
            std::ptr::read(self.data.as_ptr())
        } else {
            self.sig.assume_init()
        }
    }
    #[inline(always)]
    unsafe fn drop_local_data(&mut self) {
        if size_of::<T>() > size_of::<*mut T>() {
            self.data.assume_init_drop()
        } else {
            self.sig.load_and_drop()
        }
    }

    #[inline(always)]
    pub(crate) fn new_ref(internal: &'a Internal<T>) -> Self {
        Self {
            state: FutureState::Zero,
            sig: Signal::new_async(),
            internal,
            data: MaybeUninit::uninit(),
            is_stream: false,
            _pinned: PhantomPinned,
        }
    }
}

impl<'a, T> Future for ReceiveFuture<'a, T> {
    type Output = Result<T, ReceiveError>;

    #[inline(always)]
    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        loop {
            return match this.state {
                FutureState::Zero => {
                    let mut internal = acquire_internal(this.internal);
                    if internal.recv_count == 0 {
                        this.state = FutureState::Done;
                        return Poll::Ready(Err(ReceiveError::Closed));
                    }
                    if let Some(v) = internal.queue.pop_front() {
                        if let Some(t) = internal.next_send() {
                            // if there is a sender take its data and push it into the queue
                            unsafe { internal.queue.push_back(t.recv()) }
                        }
                        drop(internal);
                        this.state = FutureState::Done;
                        Poll::Ready(Ok(v))
                    } else if let Some(t) = internal.next_send() {
                        drop(internal);
                        this.state = FutureState::Done;
                        unsafe { Poll::Ready(Ok(t.recv())) }
                    } else {
                        if internal.send_count == 0 {
                            this.state = FutureState::Done;
                            return Poll::Ready(Err(ReceiveError::SendClosed));
                        }
                        this.state = FutureState::Waiting;
                        if size_of::<T>() > size_of::<*mut T>() {
                            // if type T smaller than register size, it does not need pointer setup as data will be stored in register address object
                            this.sig
                                .set_ptr(KanalPtr::new_unchecked(this.data.as_mut_ptr()));
                        }
                        this.sig.register_waker(cx.waker());
                        // no active waiter so push to the queue
                        internal.push_recv(this.sig.get_terminator());
                        drop(internal);
                        Poll::Pending
                    }
                }
                FutureState::Waiting => {
                    // waker is same no need to update
                    // Safety: this.sig is pinned, sig is pinned too
                    let r = this.sig.poll();
                    match r {
                        Poll::Ready(success) => {
                            this.state = FutureState::Done;
                            if success {
                                Poll::Ready(Ok(unsafe { this.read_local_data() }))
                            } else {
                                Poll::Ready(Err(ReceiveError::Closed))
                            }
                        }
                        Poll::Pending => {
                            if !this.sig.will_wake(cx.waker()) {
                                // the Waker is changed and we need to update waker in the waiting list
                                {
                                    let internal = acquire_internal(this.internal);
                                    if internal.recv_signal_exists(&this.sig) {
                                        // signal is not shared with other thread yet so it's safe to update waker locally
                                        this.sig.register_waker(cx.waker());
                                        return Poll::Pending;
                                    }
                                }
                                // the signal is already shared, and data will be available shortly, so wait synchronously and return the result
                                // note: it's not possible safely to update waker after the signal is shared, but we know data will be ready shortly,
                                //   we can wait synchronously and receive it.
                                this.state = FutureState::Done;
                                if this.sig.async_blocking_wait() {
                                    return Poll::Ready(Ok(unsafe { this.read_local_data() }));
                                }
                                return Poll::Ready(Err(ReceiveError::Closed));
                            }
                            Poll::Pending
                        }
                    }
                }
                _ => {
                    if this.is_stream {
                        this.state = FutureState::Zero;
                        continue;
                    }
                    panic!("polled after result is already returned")
                }
            };
        }
    }
}

/// Receive stream
pub struct ReceiveStream<'a, T: 'a> {
    future: Pin<Box<ReceiveFuture<'a, T>>>,
    terminated: bool,
    receiver: &'a AsyncReceiver<T>,
}
impl<'a, T> Debug for ReceiveStream<'a, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ReceiveStream {{ .. }}")
    }
}

impl<'a, T> Stream for ReceiveStream<'a, T> {
    type Item = T;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        if self.terminated {
            return Poll::Ready(None);
        }
        // Safety: future is pinned as stream is pinned to a location too
        match self.future.as_mut().poll(cx) {
            Poll::Ready(res) => match res {
                Ok(d) => Poll::Ready(Some(d)),
                Err(_) => {
                    self.terminated = true;
                    Poll::Ready(None)
                }
            },
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<'a, T> FusedStream for ReceiveStream<'a, T> {
    fn is_terminated(&self) -> bool {
        self.receiver.is_terminated()
    }
}

impl<'a, T> ReceiveStream<'a, T> {
    pub(crate) fn new_borrowed(receiver: &'a AsyncReceiver<T>) -> Self {
        let mut future = receiver.recv();
        future.is_stream = true;
        ReceiveStream {
            future: Box::pin(future),
            terminated: false,
            receiver,
        }
    }
}
