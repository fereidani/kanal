use std::{
    cell::Cell,
    future::Future,
    mem::{needs_drop, MaybeUninit},
    pin::Pin,
    task::Poll,
};

use crate::{
    internal::{acquire_internal, Internal},
    signal::AsyncSignal,
    state, Error,
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
            if !this.state.is_done() && this.state.is_waiting() {
                let mut internal = acquire_internal(this.internal);
                if !internal.cancel_send_signal(this.sig.as_signal()){
                    // someone got signal ownership, should wait until response
                    this.sig.wait_indefinitely();
                }else if needs_drop::<T>(){
                    unsafe{this.data.assume_init_drop()};
                }
            }
        }
    }
}

impl<'a, T> Future for SendFuture<'a, T> {
    type Output = Result<(), Error>;

    #[inline(always)]
    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        match *this.state {
            FutureState::Zero => {
                let mut internal = acquire_internal(this.internal);
                if internal.send_count == 0 {
                    *this.state = FutureState::Done;
                    return Poll::Ready(Err(Error::Closed));
                }
                if let Some(first) = internal.next_recv() {
                    drop(internal);
                    unsafe { first.send(std::ptr::read(this.data.as_mut_ptr())) }
                    *this.state = FutureState::Done;
                    Poll::Ready(Ok(()))
                } else if internal.queue.len() < internal.capacity {
                    internal
                        .queue
                        .push_back(unsafe { std::ptr::read(this.data.as_mut_ptr()) });
                    drop(internal);
                    *this.state = FutureState::Done;
                    Poll::Ready(Ok(()))
                } else {
                    if internal.recv_count == 0 {
                        *this.state = FutureState::Done;
                        return Poll::Ready(Err(Error::ReceiveClosed));
                    }
                    *this.state = FutureState::Waiting;
                    this.sig.set_ptr(this.data.as_mut_ptr());
                    this.sig.register(cx.waker());
                    // send directly to wait list
                    internal.push_send(this.sig.as_signal());
                    drop(internal);
                    Poll::Pending
                }
            }
            FutureState::Waiting => {
                let r = this.sig.poll(cx);
                match r {
                    Poll::Ready(v) => {
                        *this.state = FutureState::Done;
                        if v == state::UNLOCKED {
                            return Poll::Ready(Ok(()));
                        }
                        Poll::Ready(Err(Error::SendClosed))
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
            _ => {
                panic!("polled after result is already returned")
            }
        }
    }
}

pin_project! {
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
            if !this.state.is_done() && this.state.is_waiting() {
                let mut internal = acquire_internal(this.internal);
                if !internal.cancel_recv_signal(this.sig.as_signal()){
                    // someone got signal ownership, should wait until response
                    this.sig.wait_indefinitely();
                    // got ownership of data that is not gonna be used ever again, so drop it
                    if needs_drop::<T>(){
                        unsafe { std::ptr::drop_in_place(this.data.as_mut_ptr()) }
                    }
                }
            }
        }
    }
}

impl<'a, T> Future for ReceiveFuture<'a, T> {
    type Output = Result<T, Error>;

    #[inline(always)]
    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        match this.state {
            FutureState::Zero => {
                let mut internal = acquire_internal(this.internal);
                if internal.recv_count == 0 {
                    *this.state = FutureState::Done;
                    return Poll::Ready(Err(Error::Closed));
                }
                if let Some(v) = internal.queue.pop_front() {
                    if let Some(p) = internal.next_send() {
                        // if there is a sender take its data and push it in queue
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
                        return Poll::Ready(Err(Error::SendClosed));
                    }
                    *this.state = FutureState::Waiting;
                    this.sig.set_ptr(this.data.as_mut_ptr());
                    this.sig.register(cx.waker());
                    // no active waiter so push to queue
                    internal.push_recv(this.sig.as_signal());
                    drop(internal);
                    Poll::Pending
                }
            }
            FutureState::Waiting => {
                let r = this.sig.poll(cx);
                match r {
                    Poll::Ready(v) => {
                        *this.state = FutureState::Done;
                        if v == state::UNLOCKED {
                            if std::mem::size_of::<T>() == 0 {
                                return Poll::Ready(Ok(unsafe { std::mem::zeroed() }));
                            } else {
                                return Poll::Ready(Ok(unsafe {
                                    std::ptr::read(this.data.as_mut_ptr())
                                }));
                            }
                        }
                        Poll::Ready(Err(Error::ReceiveClosed))
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
            _ => {
                panic!("polled after result is already returned")
            }
        }
    }
}
