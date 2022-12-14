use std::mem::size_of;
use std::{
    future::Future,
    mem::{needs_drop, MaybeUninit},
    pin::Pin,
    task::Poll,
};

use crate::{
    internal::{acquire_internal, Internal},
    pointer::KanalPtr,
    signal::AsyncSignal,
    state, ReceiveError, SendError,
};

use pin_project_lite::pin_project;

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

pin_project! {
    /// Send future to send an object to the channel asynchronously
    /// It must be polled to perform send action
    #[must_use = "futures do nothing unless you .await or poll them"]
    pub struct SendFuture<'a, T> {
        pub(crate) state: FutureState,
        pub(crate) internal: &'a Internal<T>,
        #[pin]
        pub(crate) sig: AsyncSignal<T>,
        pub(crate) data: MaybeUninit<T>,
    }
    impl<'a,T> PinnedDrop for SendFuture<'a,T> {
        fn drop(mut this: Pin<&mut Self>) {
            if !this.state.is_done() {
                if this.state.is_waiting() && !acquire_internal(this.internal).cancel_send_signal(this.sig.as_signal()) {
                    // a receiver got signal ownership, should wait until the response
                    if this.sig.wait_indefinitely() == state::UNLOCKED {
                        // no need to drop data is moved to receiver
                        return
                    }
                }
                // signal is canceled, or in zero stated, drop data locally
                if needs_drop::<T>(){
                    // Safety: data is not moved it's safe to drop it
                    unsafe {
                        this.drop_local_data();
                    }
                }
            }
        }
    }
}

impl<'a, T> SendFuture<'a, T> {
    #[inline(always)]
    unsafe fn read_local_data(&self) -> T {
        if size_of::<T>() > size_of::<*mut T>() {
            // if its smaller than register size, it does not need pointer setup as data will be stored in register address object
            std::ptr::read(self.data.as_ptr())
        } else {
            self.sig.read_kanal_ptr()
        }
    }
    #[inline(always)]
    unsafe fn drop_local_data(&mut self) {
        if size_of::<T>() > size_of::<*mut T>() {
            unsafe { self.data.assume_init_drop() };
        } else {
            unsafe { self.sig.read_and_drop_ptr() };
        }
    }
}

impl<'a, T> Future for SendFuture<'a, T> {
    type Output = Result<(), SendError>;

    #[inline(always)]
    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let mut this = self.as_mut().project();

        match *this.state {
            FutureState::Zero => {
                let mut internal = acquire_internal(this.internal);
                if internal.recv_count == 0 {
                    let send_count = internal.send_count;
                    drop(internal);
                    *this.state = FutureState::Done;
                    if needs_drop::<T>() {
                        // the data failed to move, drop it locally
                        // Safety: the data is not moved, we are sure that it is inited in this point, it's safe to init drop it.
                        unsafe {
                            self.drop_local_data();
                        }
                    }
                    if send_count == 0 {
                        return Poll::Ready(Err(SendError::Closed));
                    }
                    return Poll::Ready(Err(SendError::ReceiveClosed));
                }
                if let Some(first) = internal.next_recv() {
                    drop(internal);
                    *this.state = FutureState::Done;
                    // Safety: data is inited and available from constructor
                    unsafe { first.send(self.read_local_data()) }
                    Poll::Ready(Ok(()))
                } else if internal.queue.len() < internal.capacity {
                    *this.state = FutureState::Done;
                    // Safety: data is inited and available from constructor
                    internal.queue.push_back(unsafe { self.read_local_data() });
                    drop(internal);
                    Poll::Ready(Ok(()))
                } else {
                    *this.state = FutureState::Waiting;
                    // if T is smaller than register size, we already have data in pointer address from initialization step
                    if size_of::<T>() > size_of::<*mut T>() {
                        this.sig
                            .set_ptr(KanalPtr::new_unchecked(this.data.as_mut_ptr()));
                    }
                    this.sig.register(cx.waker());
                    // send directly to the waitlist
                    internal.push_send(this.sig.as_signal());
                    drop(internal);
                    Poll::Pending
                }
            }
            FutureState::Waiting => {
                if this.sig.will_wake(cx.waker()) {
                    // waker is same no need to update
                    let r = this.sig.poll(cx);
                    match r {
                        Poll::Ready(v) => {
                            *this.state = FutureState::Done;
                            if v == state::UNLOCKED {
                                return Poll::Ready(Ok(()));
                            }
                            if needs_drop::<T>() {
                                // the data failed to move, drop it locally
                                // Safety: the data is not moved, we are sure that it is inited in this point, it's safe to init drop it.
                                unsafe { self.drop_local_data() };
                            }
                            Poll::Ready(Err(SendError::Closed))
                        }
                        Poll::Pending => Poll::Pending,
                    }
                } else {
                    // Waker is changed and we need to update waker in the waiting list
                    {
                        let mut internal = acquire_internal(this.internal);
                        if internal.send_signal_exists(this.sig.as_signal()) {
                            // signal is not shared with other thread yet so it's safe to update waker locally
                            this.sig.register(cx.waker());
                            return Poll::Pending;
                        }
                    }
                    // signal is already shared, and data will be available shortly, so wait synchronously and return the result
                    // note: it's not possible safely to update waker after the signal is shared, but we know data will be ready shortly,
                    //   we can wait synchronously and receive it.
                    *this.state = FutureState::Done;
                    if this.sig.wait_indefinitely() == state::UNLOCKED {
                        return Poll::Ready(Ok(()));
                    }
                    // the data failed to move, drop it locally
                    // Safety: the data is not moved, we are sure that it is inited in this point, it's safe to init drop it.
                    if needs_drop::<T>() {
                        unsafe {
                            self.drop_local_data();
                        }
                    }
                    Poll::Ready(Err(SendError::Closed))
                }
            }
            _ => {
                panic!("polled after result is already returned")
            }
        }
    }
}

pin_project! {
    /// Receive future to receive an object from the channel asynchronously
    /// It must be polled to perform receive action
    #[must_use = "futures do nothing unless you .await or poll them"]
    pub struct ReceiveFuture<'a, T> {
        pub(crate) state: FutureState,
        pub(crate) internal: &'a Internal<T>,
        #[pin]
        pub(crate) sig: AsyncSignal<T>,
        pub(crate) data: MaybeUninit<T>,
    }
    impl<'a,T> PinnedDrop for ReceiveFuture<'a,T> {
        fn drop(mut this: Pin<&mut Self>) {
            if this.state.is_waiting() {
                // try to cancel recv signal
                if !acquire_internal(this.internal).cancel_recv_signal(this.sig.as_signal()) {
                    // a sender got signal ownership, receiver should wait until the response
                    if this.sig.wait_indefinitely() == state::UNLOCKED {
                        // got ownership of data that is not going to be used ever again, so drop it
                        if needs_drop::<T>(){
                            // Safety: data is not moved it's safe to drop it
                            unsafe {
                                this.drop_local_data();
                            }
                        }
                    }
                }
            }
        }
    }
}

impl<'a, T> ReceiveFuture<'a, T> {
    #[inline(always)]
    unsafe fn read_local_data(&self) -> T {
        if size_of::<T>() > size_of::<*mut T>() {
            // if T is smaller than register size, it does not need pointer setup as data will be stored in register address object
            std::ptr::read(self.data.as_ptr())
        } else {
            self.sig.read_kanal_ptr()
        }
    }
    #[inline(always)]
    unsafe fn drop_local_data(&mut self) {
        if size_of::<T>() > size_of::<*mut T>() {
            self.data.assume_init_drop()
        } else {
            self.sig.read_and_drop_ptr()
        }
    }
}

impl<'a, T> Future for ReceiveFuture<'a, T> {
    type Output = Result<T, ReceiveError>;

    #[inline(always)]
    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let mut this = self.as_mut().project();
        match this.state {
            FutureState::Zero => {
                let mut internal = acquire_internal(this.internal);
                if internal.recv_count == 0 {
                    *this.state = FutureState::Done;
                    return Poll::Ready(Err(ReceiveError::Closed));
                }
                if let Some(v) = internal.queue.pop_front() {
                    if let Some(p) = internal.next_send() {
                        // if there is a sender take its data and push it into the queue
                        unsafe { internal.queue.push_back(p.recv()) }
                    }
                    drop(internal);
                    *this.state = FutureState::Done;
                    Poll::Ready(Ok(v))
                } else if let Some(p) = internal.next_send() {
                    drop(internal);
                    *this.state = FutureState::Done;
                    unsafe { Poll::Ready(Ok(p.recv())) }
                } else {
                    if internal.send_count == 0 {
                        *this.state = FutureState::Done;
                        return Poll::Ready(Err(ReceiveError::SendClosed));
                    }
                    *this.state = FutureState::Waiting;
                    if size_of::<T>() > size_of::<*mut T>() {
                        // if type T smaller than register size, it does not need pointer setup as data will be stored in register address object
                        this.sig
                            .set_ptr(KanalPtr::new_unchecked(this.data.as_mut_ptr()));
                    }
                    this.sig.register(cx.waker());
                    // no active waiter so push to the queue
                    internal.push_recv(this.sig.as_signal());
                    drop(internal);
                    Poll::Pending
                }
            }
            FutureState::Waiting => {
                if this.sig.will_wake(cx.waker()) {
                    // waker is same no need to update
                    let r = this.sig.poll(cx);
                    match r {
                        Poll::Ready(v) => {
                            *this.state = FutureState::Done;
                            if v == state::UNLOCKED {
                                return Poll::Ready(Ok(unsafe { self.read_local_data() }));
                            }
                            Poll::Ready(Err(ReceiveError::Closed))
                        }
                        Poll::Pending => Poll::Pending,
                    }
                } else {
                    // the Waker is changed and we need to update waker in the waiting list
                    {
                        let mut internal = acquire_internal(this.internal);
                        if internal.recv_signal_exists(this.sig.as_signal()) {
                            // signal is not shared with other thread yet so it's safe to update waker locally
                            this.sig.register(cx.waker());
                            return Poll::Pending;
                        }
                    }
                    // the signal is already shared, and data will be available shortly, so wait synchronously and return the result
                    // note: it's not possible safely to update waker after the signal is shared, but we know data will be ready shortly,
                    //   we can wait synchronously and receive it.
                    if this.sig.wait_indefinitely() == state::UNLOCKED {
                        return Poll::Ready(Ok(unsafe { self.read_local_data() }));
                    }
                    Poll::Ready(Err(ReceiveError::Closed))
                }
            }
            _ => {
                panic!("polled after result is already returned")
            }
        }
    }
}
