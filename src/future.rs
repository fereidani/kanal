use crate::{
    internal::{acquire_internal, Internal},
    signal::AsyncSignal,
    AsyncReceiver, ReceiveError, SendError,
};
use core::{cell::UnsafeCell, fmt::Debug, marker::PhantomPinned, pin::Pin, task::Poll};

use branches::{likely, unlikely};
use futures_core::{FusedStream, Future, Stream};

#[repr(u8)]
#[derive(PartialEq, Clone, Copy)]
pub(crate) enum FutureState {
    Unregistered,
    Pending,
    Success,
    Failure,
    Done,
}

#[cold]
fn mark_branch_unlikely() {}

#[allow(unused)]
impl FutureState {
    #[inline(always)]
    fn is_pending(&self) -> bool {
        *self == FutureState::Pending
    }

    #[inline(always)]
    fn is_done(&self) -> bool {
        *self == FutureState::Done
    }

    #[inline(always)]
    fn is_unregistered(&self) -> bool {
        *self == FutureState::Unregistered
    }

    #[inline(always)]
    fn is_success(&self) -> bool {
        *self == FutureState::Success
    }

    #[inline(always)]
    fn is_failure(&self) -> bool {
        *self == FutureState::Failure
    }
}

/// SendFuture is a future for sending an object to a channel asynchronously.
/// It must be polled to complete the send operation.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct SendFuture<'a, T> {
    internal: &'a Internal<T>,
    sig: AsyncSignal<T>,
    _pinned: PhantomPinned,
}

impl<T> Debug for SendFuture<'_, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SendFuture {{ .. }}")
    }
}

impl<T> Drop for SendFuture<'_, T> {
    fn drop(&mut self) {
        let state = self.sig.state();
        if unlikely(!state.is_done()) {
            // If we are still pending, try to cancel the send operation.
            // Cancellation succeeds → we still own the data and must drop it.
            // Otherwise the receiver may already own the payload; we wait for it
            // to finish before dropping the future.
            let mut need_drop = true;

            if state.is_pending()
                && acquire_internal(self.internal).cancel_send_signal(self.sig.as_tagged_ptr())
            {
                // Cancellation succeeded – we still own the data.
                need_drop = true;
            } else if !state.is_unregistered() && self.sig.blocking_wait() {
                // A receiver has taken ownership; it will drop the data.
                need_drop = false;
            }

            if need_drop {
                // SAFETY: the payload has never been moved out of the signal.
                unsafe { self.sig.drop_data() };
            }
        }
    }
}

impl<'a, T> SendFuture<'a, T> {
    /// Creates a new SendFuture with the given internal channel and data.
    #[inline(always)]
    pub(crate) fn new(internal: &'a Internal<T>, data: T) -> Self {
        SendFuture {
            internal,
            sig: AsyncSignal::new_send(data),
            _pinned: PhantomPinned,
        }
    }
    #[inline(always)]
    pub(crate) fn new_finished(internal: &'a Internal<T>) -> Self {
        SendFuture {
            internal,
            sig: AsyncSignal::new_send_finished(),
            _pinned: PhantomPinned,
        }
    }
}

impl<T> Future for SendFuture<'_, T> {
    type Output = Result<(), SendError<T>>;

    #[inline(always)]
    fn poll(self: Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        match this.sig.state() {
            FutureState::Unregistered => {
                let cap = this.internal.capacity();
                let mut internal = acquire_internal(this.internal);
                if unlikely(internal.recv_count == 0) {
                    drop(internal);
                    this.sig.set_state_relaxed(FutureState::Done);
                    // SAFETY: the data failed to move, we can safely return it to user
                    unsafe {
                        return Poll::Ready(Err(SendError(this.sig.assume_init())));
                    }
                }
                if let Some(first) = internal.next_recv() {
                    drop(internal);
                    this.sig.set_state_relaxed(FutureState::Done);
                    // SAFETY: data is inited and available from constructor
                    unsafe { first.send(this.sig.assume_init()) }
                    return Poll::Ready(Ok(()));
                }
                if cap > 0 && internal.queue.len() < cap {
                    this.sig.set_state_relaxed(FutureState::Done);
                    // SAFETY: data is inited and available from constructor
                    internal.queue.push_back(unsafe { this.sig.assume_init() });
                    drop(internal);
                    return Poll::Ready(Ok(()));
                }
                this.sig.set_state(FutureState::Pending);
                // SAFETY: waker is empty, it is safe to init it here
                unsafe {
                    this.sig.update_waker(cx.waker());
                }
                // send directly to the waitlist
                internal.push_signal(this.sig.dynamic_ptr());
                drop(internal);
                Poll::Pending
            }
            FutureState::Success => {
                this.sig.set_state_relaxed(FutureState::Done);
                Poll::Ready(Ok(()))
            }
            FutureState::Pending => {
                mark_branch_unlikely();
                let waker = cx.waker();
                // SAFETY: signal waker is valid as we inited it in future pending state
                if unlikely(unsafe { !this.sig.will_wake(waker) }) {
                    // Waker is changed and we need to update waker in the waiting list
                    let internal = acquire_internal(this.internal);
                    if internal.send_signal_exists(this.sig.as_tagged_ptr()) {
                        // SAFETY: signal is not shared with other thread yet so it's safe to
                        // update waker locally
                        unsafe {
                            this.sig.update_waker(waker);
                        }
                        drop(internal);
                        return Poll::Pending;
                    }
                    drop(internal);
                    // signal is already shared, and data will be available shortly, so wait
                    // synchronously and return the result note:
                    // it's not possible safely to update waker after the signal is shared,
                    // but we know data will be ready shortly,
                    //   we can wait synchronously and receive it.
                    this.sig.set_state(FutureState::Done);
                    if likely(this.sig.blocking_wait()) {
                        return Poll::Ready(Ok(()));
                    }
                    // the data failed to move, we can safely return it to user
                    Poll::Ready(Err(SendError(unsafe { this.sig.assume_init() })))
                } else {
                    Poll::Pending
                }
            }
            FutureState::Failure => {
                mark_branch_unlikely();
                this.sig.set_state_relaxed(FutureState::Done);
                // SAFETY: the data failed to move, we can safely return it to user
                Poll::Ready(Err(SendError(unsafe { this.sig.assume_init() })))
            }
            FutureState::Done => {
                mark_branch_unlikely();
                panic!("polled after result is already returned")
            }
        }
    }
}

/// ReceiveFuture is a future for receiving an object from a channel
/// asynchronously. It must be polled to complete the receive operation.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct ReceiveFuture<'a, T> {
    is_stream: bool,
    internal: &'a Internal<T>,
    sig: AsyncSignal<T>,
    _pinned: PhantomPinned,
}

impl<T> Drop for ReceiveFuture<'_, T> {
    fn drop(&mut self) {
        let state = self.sig.state();
        if unlikely(!state.is_done()) {
            // try to cancel the signal if we are still waiting
            if state.is_pending()
                && acquire_internal(self.internal).cancel_recv_signal(self.sig.as_tagged_ptr())
            {
                // signal canceled
                return;
            }
            // we failed to cancel the signal,
            // either it is unregistered or a sender got signal ownership, receiver should
            // wait until the response
            if !state.is_unregistered() && self.sig.blocking_wait() {
                // got ownership of data that is not going to be used ever again, so drop it
                // this is actually a bug in user code but we should handle it gracefully
                // and we warn user in debug mode
                // SAFETY: data is not moved it's safe to drop it or put it back to the channel
                // queue
                unsafe {
                    if self.internal.capacity() == 0 {
                        #[cfg(debug_assertions)]
                        println!(
                            "warning: ReceiveFuture dropped while send operation is in progress"
                        );
                        self.sig.drop_data();
                    } else {
                        // fallback: push it back to the channel queue
                        acquire_internal(self.internal)
                            .queue
                            .push_front(self.sig.assume_init())
                    }
                }
            }
        }
    }
}

impl<T> Debug for ReceiveFuture<'_, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "ReceiveFuture {{ .. }}")
    }
}

impl<'a, T> ReceiveFuture<'a, T> {
    #[inline(always)]
    pub(crate) fn new_ref(internal: &'a Internal<T>) -> Self {
        Self {
            sig: AsyncSignal::new_recv(),
            internal,
            is_stream: false,
            _pinned: PhantomPinned,
        }
    }
}

impl<T> Future for ReceiveFuture<'_, T> {
    type Output = Result<T, ReceiveError>;

    #[inline(always)]
    fn poll(self: Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        loop {
            return match this.sig.state() {
                FutureState::Unregistered => {
                    let cap = this.internal.capacity();
                    let mut internal = acquire_internal(this.internal);
                    if unlikely(internal.recv_count == 0) {
                        drop(internal);
                        this.sig.set_state_relaxed(FutureState::Done);
                        return Poll::Ready(Err(ReceiveError()));
                    }
                    if cap > 0 {
                        if let Some(v) = internal.queue.pop_front() {
                            if let Some(t) = internal.next_send() {
                                // if there is a sender take its data and push it into the queue
                                unsafe { internal.queue.push_back(t.recv()) }
                            }
                            drop(internal);
                            this.sig.set_state_relaxed(FutureState::Done);
                            return Poll::Ready(Ok(v));
                        }
                    }
                    if let Some(t) = internal.next_send() {
                        drop(internal);
                        this.sig.set_state_relaxed(FutureState::Done);
                        return Poll::Ready(Ok(unsafe { t.recv() }));
                    }
                    if unlikely(internal.send_count == 0) {
                        this.sig.set_state_relaxed(FutureState::Done);
                        return Poll::Ready(Err(ReceiveError()));
                    }
                    this.sig.set_state(FutureState::Pending);
                    // SAFETY: waker is NOOP and not shared yet, it is safe to init it here
                    unsafe {
                        this.sig.update_waker(cx.waker());
                    }
                    // no active waiter so push to the queue
                    internal.push_signal(this.sig.dynamic_ptr());
                    drop(internal);
                    Poll::Pending
                }
                FutureState::Success => {
                    this.sig.set_state_relaxed(FutureState::Done);
                    // SAFETY: data is received and safe to read
                    Poll::Ready(Ok(unsafe { this.sig.assume_init() }))
                }
                FutureState::Pending => {
                    let waker = cx.waker();
                    // SAFETY: signal waker is valid as we inited it in future pending state
                    if unsafe { !this.sig.will_wake(waker) } {
                        // the Waker is changed and we need to update waker in the waiting
                        // list
                        let internal = acquire_internal(this.internal);
                        if internal.recv_signal_exists(this.sig.as_tagged_ptr()) {
                            // SAFETY: signal is not shared with other thread yet so it's safe to
                            // update waker locally
                            unsafe {
                                this.sig.update_waker(waker);
                            }
                            drop(internal);
                            Poll::Pending
                        } else {
                            drop(internal);
                            // the signal is already shared, and data will be available shortly,
                            // so wait synchronously and return the result
                            // note: it's not possible safely to update waker after the signal
                            // is shared, but we know data will be ready shortly,
                            //   we can wait synchronously and receive it.
                            this.sig.set_state_relaxed(FutureState::Done);
                            if likely(this.sig.blocking_wait()) {
                                // SAFETY: data is received and safe to read
                                Poll::Ready(Ok(unsafe { this.sig.assume_init() }))
                            } else {
                                Poll::Ready(Err(ReceiveError()))
                            }
                        }
                    } else {
                        Poll::Pending
                    }
                }
                FutureState::Failure => {
                    mark_branch_unlikely();
                    this.sig.set_state_relaxed(FutureState::Done);
                    Poll::Ready(Err(ReceiveError()))
                }
                FutureState::Done => {
                    mark_branch_unlikely();
                    if this.is_stream {
                        this.sig.set_state_relaxed(FutureState::Unregistered);
                        continue;
                    }
                    panic!("polled after result is already returned")
                }
            };
        }
    }
}

/// ReceiveStream is a stream for receiving objects from a channel
/// asynchronously.
pub struct ReceiveStream<'a, T: 'a> {
    future: Pin<Box<ReceiveFuture<'a, T>>>,
    terminated: bool,
    receiver: &'a AsyncReceiver<T>,
}

impl<T> Debug for ReceiveStream<'_, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "ReceiveStream {{ .. }}")
    }
}

impl<T> Stream for ReceiveStream<'_, T> {
    type Item = T;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        if unlikely(self.terminated) {
            return Poll::Ready(None);
        }
        // SAFETY: future is pinned as stream is pinned to a location too
        match self.future.as_mut().poll(cx) {
            Poll::Ready(res) => match res {
                Ok(d) => Poll::Ready(Some(d)),
                Err(_) => {
                    mark_branch_unlikely();
                    self.terminated = true;
                    Poll::Ready(None)
                }
            },
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T> FusedStream for ReceiveStream<'_, T> {
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

/// SendManyFuture is a future for sending multiple objects to a channel
/// asynchronously. It must be polled to complete the send operation.
#[must_use = "futures do nothing unless you .await or poll them"]
#[derive(Debug)]
pub struct SendManyFuture<'a, 'b, T> {
    internal: &'a Internal<T>,
    // This is a UnsafeCell, because it can be shared in WaitQueue of the channel
    fut: UnsafeCell<SendFuture<'a, T>>,
    // Elements that we are writing to the channel
    elements: &'b mut std::collections::VecDeque<T>,
    // Future is finished and no longer should be polled
    finished: bool,
    // If set we are in wait queue and fut shall not be used as mutable
    in_wait_queue: bool,
    _pinned: PhantomPinned,
}

impl<'a, 'b, T> SendManyFuture<'a, 'b, T> {
    #[inline(always)]
    pub(crate) fn new(
        internal: &'a Internal<T>,
        elements: &'b mut std::collections::VecDeque<T>,
    ) -> Self {
        SendManyFuture {
            internal,
            fut: UnsafeCell::new(SendFuture::new_finished(internal)),
            elements,
            finished: false,
            in_wait_queue: false,
            _pinned: PhantomPinned,
        }
    }
}

impl<'a, 'b, T> Future for SendManyFuture<'a, 'b, T> {
    type Output = Result<(), SendError<T>>;

    #[inline(always)]
    fn poll(self: Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        loop {
            if unlikely(this.finished) {
                panic!("polled after completion");
            }

            if this.in_wait_queue {
                // SAFETY: this is pinned therefore it's safe to create a pinned reference
                let fut = unsafe { Pin::new_unchecked(this.fut.get_mut()) };

                match fut.poll(cx) {
                    Poll::Ready(res) => {
                        if this.elements.is_empty() {
                            this.finished = true;
                            return Poll::Ready(res);
                        }
                    }
                    Poll::Pending => return Poll::Pending,
                }
                this.in_wait_queue = false;
            }

            // If there is nothing to send we are done.
            if unlikely(this.elements.is_empty()) {
                this.finished = true;
                return Poll::Ready(Ok(()));
            }

            // Channel capacity
            let cap = this.internal.capacity();

            // Acquire a mutable reference to the internal state.
            let mut internal = acquire_internal(this.internal);

            // Channel is closed from the other side
            if unlikely(internal.recv_count == 0) {
                // Return the first element that could not be sent.
                let first = this.elements.pop_front().unwrap();
                drop(internal);
                this.finished = true;
                return Poll::Ready(Err(SendError(first)));
            }

            // -----------------------------------------------------------------
            // 1) Deliver directly to waiting receivers.
            // -----------------------------------------------------------------
            while let Some(waiter) = internal.next_recv() {
                let v = this.elements.pop_front().unwrap();
                // SAFETY: it is safe to send an owned waiter once
                unsafe {
                    waiter.send(v);
                }
                if unlikely(this.elements.is_empty()) {
                    // No more elements to send.
                    drop(internal);
                    this.finished = true;
                    return Poll::Ready(Ok(()));
                }
            }

            if unlikely(this.elements.is_empty()) {
                // Nothing left to send.
                drop(internal);
                this.finished = true;
                return Poll::Ready(Ok(()));
            }

            // -----------------------------------------------------------------
            // 2) Fill the channel’s queue (if it has a bounded/unbounded capacity).
            // -----------------------------------------------------------------
            if cap > 0 {
                while internal.queue.len() < cap {
                    if let Some(v) = this.elements.pop_front() {
                        internal.queue.push_back(v);
                    } else {
                        // All elements have been queued.
                        drop(internal);
                        this.finished = true;
                        return Poll::Ready(Ok(()));
                    }
                }
            }

            // -----------------------------------------------------------------
            // 3) If there are still elements, send the next one via a signal
            // -----------------------------------------------------------------
            if let Some(v) = this.elements.pop_front() {
                // SAFETY: we checked above and we are not in any wait queue as we are not
                // registered in the queue yet.
                unsafe {
                    this.fut.get_mut().sig.reset_send(v);
                }
                // This is pinned therefore this.fut is also pinned.
                internal.push_signal(this.fut.get_mut().sig.dynamic_ptr());

                this.in_wait_queue = true;
                drop(internal);
                // go poll the future to register the waker or return early if message already
                // arrived
                continue;
            } else {
                // No more elements left.
                this.finished = true;
                return Poll::Ready(Ok(()));
            }
        }
    }
}

/// DrainIntoBlockingFuture is a future for draining all available messages from a channel
/// into a vector, blocking until at least one message is received.
#[must_use = "futures do nothing unless you .await or poll them"]
pub struct DrainIntoBlockingFuture<'a, 'b, T> {
    internal: &'a Internal<T>,
    sig: AsyncSignal<T>,
    vec: &'b mut Vec<T>,
    _pinned: PhantomPinned,
}

impl<T> Drop for DrainIntoBlockingFuture<'_, '_, T> {
    fn drop(&mut self) {
        let state = self.sig.state();
        if unlikely(!state.is_done()) {
            // try to cancel the signal if we are still waiting
            if state.is_pending()
                && acquire_internal(self.internal).cancel_recv_signal(self.sig.as_tagged_ptr())
            {
                // signal canceled
                return;
            }
            // we failed to cancel the signal,
            // either it is unregistered or a sender got signal ownership, receiver should
            // wait until the response
            if !state.is_unregistered() && self.sig.blocking_wait() {
                // got ownership of data that is not going to be used ever again, so drop it
                // SAFETY: data is not moved it's safe to drop it or put it back to the channel queue
                unsafe {
                    if self.internal.capacity() == 0 {
                        self.sig.drop_data();
                    } else {
                        // fallback: push it back to the channel queue
                        acquire_internal(self.internal)
                            .queue
                            .push_front(self.sig.assume_init())
                    }
                }
            }
        }
    }
}

impl<T> Debug for DrainIntoBlockingFuture<'_, '_, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "DrainIntoBlockingFuture {{ .. }}")
    }
}

impl<'a, 'b, T> DrainIntoBlockingFuture<'a, 'b, T> {
    #[inline(always)]
    pub(crate) fn new(internal: &'a Internal<T>, vec: &'b mut Vec<T>) -> Self {
        Self {
            sig: AsyncSignal::new_recv(),
            internal,
            vec,
            _pinned: PhantomPinned,
        }
    }
}

impl<T> Future for DrainIntoBlockingFuture<'_, '_, T> {
    type Output = Result<usize, ReceiveError>;

    #[inline(always)]
    fn poll(self: Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        match this.sig.state() {
            FutureState::Unregistered => {
                let vec_initial_length = this.vec.len();
                let mut internal = acquire_internal(this.internal);

                // Check if channel is closed
                if unlikely(internal.recv_count == 0) {
                    this.sig.set_state_relaxed(FutureState::Done);
                    return Poll::Ready(Err(ReceiveError()));
                }

                // Calculate required capacity and reserve
                let required_cap = internal.queue.len() + {
                    if internal.recv_blocking {
                        0
                    } else {
                        internal.wait_list.len()
                    }
                };
                let remaining_cap = this.vec.capacity() - vec_initial_length;
                if required_cap > remaining_cap {
                    this.vec
                        .reserve(vec_initial_length + required_cap - remaining_cap);
                }

                // Drain queue
                this.vec.extend(internal.queue.drain(..));

                // Drain wait_list send signals
                while let Some(p) = internal.next_send() {
                    // SAFETY: it's safe to receive from owned signal once
                    unsafe { this.vec.push(p.recv()) }
                }

                // If got data, return immediately
                let count = this.vec.len() - vec_initial_length;
                if count > 0 {
                    this.sig.set_state_relaxed(FutureState::Done);
                    return Poll::Ready(Ok(count));
                }

                // No data, check if there are still senders
                if unlikely(internal.send_count == 0) {
                    this.sig.set_state_relaxed(FutureState::Done);
                    return Poll::Ready(Err(ReceiveError()));
                }

                // Register signal and wait
                this.sig.set_state(FutureState::Pending);
                // SAFETY: waker is NOOP and not shared yet, it is safe to init it here
                unsafe {
                    this.sig.update_waker(cx.waker());
                }
                internal.push_signal(this.sig.dynamic_ptr());
                drop(internal);
                Poll::Pending
            }
            FutureState::Success => {
                this.sig.set_state_relaxed(FutureState::Done);
                // SAFETY: data is received and safe to read
                this.vec.push(unsafe { this.sig.assume_init() });
                Poll::Ready(Ok(1))
            }
            FutureState::Pending => {
                let waker = cx.waker();
                // SAFETY: signal waker is valid as we inited it in future pending state
                if unsafe { !this.sig.will_wake(waker) } {
                    // the Waker is changed and we need to update waker in the waiting list
                    let internal = acquire_internal(this.internal);
                    if internal.recv_signal_exists(this.sig.as_tagged_ptr()) {
                        // SAFETY: signal is not shared with other thread yet so it's safe to
                        // update waker locally
                        unsafe {
                            this.sig.update_waker(waker);
                        }
                        drop(internal);
                        Poll::Pending
                    } else {
                        drop(internal);
                        // the signal is already shared, and data will be available shortly,
                        // so wait synchronously and return the result
                        this.sig.set_state_relaxed(FutureState::Done);
                        if likely(this.sig.blocking_wait()) {
                            // SAFETY: data is received and safe to read
                            this.vec.push(unsafe { this.sig.assume_init() });
                            Poll::Ready(Ok(1))
                        } else {
                            Poll::Ready(Err(ReceiveError()))
                        }
                    }
                } else {
                    Poll::Pending
                }
            }
            FutureState::Failure => {
                mark_branch_unlikely();
                this.sig.set_state_relaxed(FutureState::Done);
                Poll::Ready(Err(ReceiveError()))
            }
            FutureState::Done => {
                mark_branch_unlikely();
                panic!("polled after result is already returned")
            }
        }
    }
}
