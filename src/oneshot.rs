/// Kanal one-shot implementation is a lock-free algorithm with only a usize
/// allocation in heap. Algorithm serializes states of the oneshot channel based
/// on pointer address in the heap, and also manages memory free operation with
/// same states, without a need to Arc.
///
/// There is a main difference between Kanal implementation of oneshot and other
/// existing implementations. Kanal synchronizes on both send and recv, while in
/// other implementations send could not be blocking/suspending, in Kanal
/// if sender reaches `send` function before receiver calls its `recv`, send
/// will block/suspend until transaction is done. In async API send will not
/// send data unless code awaits the returning future.
///
/// Channel states are simple,
/// WAITING = null pointer
/// FINISHED = full 1 bits pattern of the pointer
/// ANY OTHER ADDRESS = pointer to winner signal address
///
/// Both side of sender and receiver can know which is the winner by a simple
/// fact. If winner is not a pointer to their signal, they are the one that
/// should comply with other signal. In the other word it's like that other
/// state of channel that is which side is the winner is encoded in the
/// instruction pointer. In certain blocks WAITING means loser and in others
/// means winner.
///
/// Both side in start are racing to win the pointer, if one loses, they can
/// find the correct state of channel based on pointer address. If it is a
/// pointer to a memory location that is not FINISHED, other thread/coroutine is
/// suspended on its signal and loser should finish the signal. If it is a
/// pointer with value of FINISHED, channel is dropped from other side, and
/// operation is failed.
///
/// When loser side wants to finish the waiting signal, it should change the
/// pointer address of waiting signal to finished before finishing the
/// transaction. The reason is to avoid any mid transfer cancelation by owner of
/// signal, after change of waiting signal to finished state, under no
/// circumtances transaction cannot be canceled. Unless the owner changes its
/// own signal to FINISHED first, and cancel the transaction by itself.
/// In simple terms, won signal can change to FINISHED state by loser or winner,
/// if loser changes it, it means transaction is finishing and winner should
/// commit to finishing the transaction, and if loser changes it, it implies
/// that loser is canceling the operation completely.
///
/// Memory management is simple, there is no finishing state that pointer does
/// not change to FINISHED, and first one to reach that state is going to leave
/// the pointer for latest owner of channel either sender or receiver to free
/// the heep allocation.
///
/// Write, read and termination are done through the same [`Signal`] that is
/// backbone of Kanal MPMC channels. for more information check `signal.rs`.

#[cfg(feature = "async")]
use crate::FutureState;
use crate::{pointer::KanalPtr, signal::*, OneshotReceiveError};
#[cfg(feature = "async")]
use futures_core::Future;
use std::{
    fmt::Debug,
    marker::PhantomData,
    mem::{forget, size_of, MaybeUninit},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
#[cfg(feature = "async")]
use std::{
    marker::PhantomPinned,
    mem::{needs_drop, transmute},
    task::Poll,
};

const WAITING: *mut () = ptr::null_mut::<()>();
const FINISHED: *mut () = !0usize as *mut ();

/// [`ActionResult`] translates state of compare_exchange off the AtomicPtr to
/// more readable and easier to handle enum.
enum ActionResult<T> {
    /// Action was successfull
    Ok,
    /// State is racing, side can participate in race to win signal ptr
    Racing,
    /// The side that receiving this value is the loser, and address of winner
    /// is returned.
    Winner(*const Signal<T>),
    /// Channel is finished, either dropped or in finishing stages, actual state
    /// of channel should be check in the winner signal. If there is no
    /// winner, other side of channel is dropped.
    Finish,
}

impl<T> ActionResult<T> {
    fn is_ok(&self) -> bool {
        matches!(self, ActionResult::Ok)
    }
}

impl<T> From<Result<*mut Signal<T>, *mut Signal<T>>> for ActionResult<T> {
    #[inline(always)]
    fn from(value: Result<*mut Signal<T>, *mut Signal<T>>) -> Self {
        match value {
            Ok(_) => ActionResult::Ok,
            Err(ptr) => {
                if ptr == WAITING as *mut Signal<T> {
                    ActionResult::Racing
                } else if ptr == FINISHED as *mut Signal<T> {
                    ActionResult::Finish
                } else {
                    ActionResult::Winner(ptr as *const Signal<T>)
                }
            }
        }
    }
}

struct OneshotInternal<T> {
    ptr: AtomicPtr<Signal<T>>,
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
unsafe impl<T: Send> Send for OneshotInternalPointer<T> {}

impl<T> OneshotInternal<T> {
    /// Tries to inform other side that signal is ready. If it's done
    /// successfully it means that performer will wait for otherside to
    /// awake its signal about task finishing state which is either finished
    /// or terminated.
    #[inline(always)]
    fn try_win_race(&self, own_ptr: *const Signal<T>) -> ActionResult<T> {
        self.ptr
            .compare_exchange(
                WAITING as *mut Signal<T>,
                own_ptr as *mut Signal<T>,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .into()
    }
    /// Tries to cancel the operation for both sides,
    /// It will update the state from racing with no winner to finishing,
    /// Which will be interpreted as termination of channel for other side.
    #[inline(always)]
    fn try_terminate(&self) -> ActionResult<T> {
        self.ptr
            .compare_exchange(
                WAITING as *mut Signal<T>,
                FINISHED as *mut Signal<T>,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .into()
    }
    /// It's only used when the owner of signal in async context wants to update
    /// its waker  as signal is shared in that context, it's not safe to
    /// write to the signal as there will be multiple  mutable reference to
    /// signal, with this method channel resets state to racing if it's
    /// possible and tries to win the race again after updating the signal,
    /// the logic is something similiar to the Mutex.
    #[inline(always)]
    #[cfg(feature = "async")]
    fn try_reset(&self, own_ptr: *const Signal<T>) -> ActionResult<T> {
        self.ptr
            .compare_exchange(
                own_ptr as *mut Signal<T>,
                WAITING as *mut Signal<T>,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .into()
    }
    /// Tries to upgrade state from waiting to finish,
    /// If winer use it, it's a signal for termination of signal,
    /// If loser use it, it's a signal for trying to start the data movement.
    #[inline(always)]
    fn try_finish(&self, from: *const Signal<T>) -> ActionResult<T> {
        self.ptr
            .compare_exchange(
                from as *mut Signal<T>,
                FINISHED as *mut Signal<T>,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .into()
    }
}

// Returns true if transfer was successfull.
#[inline(never)]
fn try_drop_internal<T>(internal_ptr: OneshotInternalPointer<T>) -> bool {
    let internal = unsafe { internal_ptr.as_ref() };
    loop {
        // Safety: other side can't drop internal, if this side is not in
        // finishing stage
        match internal.try_terminate() {
            ActionResult::Ok => {
                // Signal state is updated to finishing other side is
                // responsible for dropping the internal
                return false;
            }
            ActionResult::Racing => {
                unreachable!("already tried updating from this state")
            }
            ActionResult::Winner(winner) => {
                if internal.try_finish(winner).is_ok() {
                    // Safety: as status is waiting we know that we lost the
                    // race, so recv_signal is valid.
                    unsafe { SignalTerminator::from(winner).terminate() };
                    return false;
                }
                // CONTINUE
            }
            ActionResult::Finish => {
                // Safety: other side updated status to finishing before, it's
                // safe to release memory now.
                unsafe { internal_ptr.drop() }
                return true;
            }
        }
    }
}

/// [`OneshotSender<T>`] is the sender side of oneshot channel that can send a
/// single message. It can be converted to async [`OneshotAsyncSender<T>`] by
/// calling [`Self::to_async`]. Sending a message is achievable with
/// [`Self::send`].
///
/// Note: sending a message in Kanal one-shot algorithm can be
/// blocking/suspending if the sender reaches the `send` function before
/// receiver calling the recv.
pub struct OneshotSender<T> {
    internal_ptr: OneshotInternalPointer<T>,
}

impl<T> Debug for OneshotSender<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OneshotSender {{ .. }}")
    }
}

impl<T> Drop for OneshotSender<T> {
    fn drop(&mut self) {
        try_drop_internal(self.internal_ptr);
    }
}

impl<T> OneshotSender<T> {
    /// Sends a message to the channel and consumes the send side
    /// Unlike other rust's one-shot channel implementations this function could
    /// be blocking/suspending only if the sender reaches the function
    /// before receiver calling its recv function. In case of successful
    /// operation this function returns `Ok(())` and in case of failure it
    /// returns back the sending object as `Err(T)`.
    #[inline(always)]
    pub fn send(self, data: T) -> Result<(), T> {
        let mut data = MaybeUninit::new(data);
        // Safety: other side can't drop internal, if this side is not in
        // finishing stage.
        let internal = unsafe { self.internal_ptr.as_ref() };
        let sig = Signal::new_sync(KanalPtr::new_from(data.as_mut_ptr()));
        loop {
            match internal.try_win_race(&sig) {
                ActionResult::Ok => {
                    let success = sig.wait();
                    unsafe {
                        // Safety: as convention specifies, winner or last
                        // leaving owner should drop the
                        // ptr.
                        self.internal_ptr.drop();
                        forget(self);
                    }
                    if !success {
                        // Safety: data is inited for sure and is the only
                        // single copy of the object.
                        return Err(unsafe { data.assume_init() });
                    }
                    return Ok(());
                }
                ActionResult::Racing => {
                    unreachable!("already tried updating from this state")
                }
                ActionResult::Winner(receiver) => {
                    match internal.try_finish(receiver) {
                        ActionResult::Ok => {
                            // Safety: receive signal is guaranteed to be valid due to failed race
                            // situation
                            unsafe {
                                SignalTerminator::from(receiver).send_copy(data.as_ptr());
                            }
                            // Other side is responsible for dropping
                            forget(self);
                            return Ok(());
                        }
                        ActionResult::Racing => {
                            // Race is on again try to win the signal if it's
                            // possible.
                            continue;
                        }
                        ActionResult::Finish => {
                            // otherside dropped itself
                            unsafe {
                                // Safety: as convention specifies, winner or last leaving owner
                                // should drop the ptr.
                                self.internal_ptr.drop();
                                forget(self);
                            }
                            // Safety: data is inited for sure and is the only
                            // single copy of the object.
                            return Err(unsafe { data.assume_init() });
                        }
                        ActionResult::Winner(_) => {
                            // Winner can't change, because if it's not the sender, it's receiver
                            // for sure, and address of receiver can't change.
                            unreachable!();
                        }
                    }
                }
                ActionResult::Finish => {
                    unsafe {
                        // Safety: as convention specifies, winner or last leaving owner should drop
                        // the ptr.
                        self.internal_ptr.drop();
                        forget(self);
                    }
                    // Safety: data is inited for sure and is the only single
                    // copy of the object.
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
/// [`OneshotAsyncSender<T>`] is the sender side of oneshot channel that can
/// send a single message asynchronously. It can be converted to
/// [`OneshotSender<T>`] by calling [`Self::to_sync`]. Sending a message is
/// achievable with [`Self::send`] which returns a future that should be polled
/// until transfer is done.
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
        try_drop_internal(self.internal_ptr);
    }
}

/// [`OneshotReceiver<T>`] is the receiver side of oneshot channel that can
/// receive a single message. It can be converted to async
/// [`OneshotAsyncReceiver<T>`] by calling [`Self::to_async`]. Receiving a
/// message is achievable with [`Self::recv`].
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
        try_drop_internal(self.internal_ptr);
    }
}

impl<T> OneshotReceiver<T> {
    /// Receives a message from the channel and consumes the receive side.
    #[inline(always)]
    pub fn recv(self) -> Result<T, OneshotReceiveError> {
        // Safety: other side can't drop internal, if this side is not in
        // finishing stage
        let internal = unsafe { self.internal_ptr.as_ref() };
        let mut ret = MaybeUninit::<T>::uninit();
        let sig = Signal::new_sync(KanalPtr::new_write_address_ptr(ret.as_mut_ptr()));
        loop {
            match internal.try_win_race(&sig) {
                ActionResult::Ok => {
                    // wait synchronously until data is received.
                    let success = sig.wait();
                    unsafe {
                        // Safety: as convention specifies, winner or last leaving owner should drop
                        // the ptr
                        self.internal_ptr.drop();
                        forget(self);
                    }
                    if !success {
                        return Err(OneshotReceiveError());
                    }
                    // Safety: it's safe to assume init as data is forgotten on
                    // another side
                    return if size_of::<T>() > size_of::<*mut T>() {
                        Ok(unsafe { ret.assume_init() })
                    } else {
                        Ok(unsafe { sig.assume_init() })
                    };
                }
                ActionResult::Racing => {
                    unreachable!("already tried updating from this state")
                }
                ActionResult::Winner(sender) => {
                    match internal.try_finish(sender) {
                        ActionResult::Ok => {
                            // Other side is responsible for dropping
                            forget(self);
                            // Safety: signal is valid and recv will be called only once as this
                            // call will consume the self
                            return Ok(unsafe { SignalTerminator::from(sender).recv() });
                        }
                        ActionResult::Racing => {
                            // Race is on again try to win the signal if it's
                            // possible
                            continue;
                        }
                        ActionResult::Finish => {
                            unsafe {
                                // Safety: as convention specifies, winner or last leaving owner
                                // should drop the ptr
                                self.internal_ptr.drop();
                                forget(self);
                            }
                            return Err(OneshotReceiveError());
                        }
                        ActionResult::Winner(_) => {
                            // Winner address can't change
                            unreachable!("winner address can't change");
                        }
                    }
                }
                ActionResult::Finish => {
                    unsafe {
                        // Safety: as convention specifies, winner or last leaving owner should drop
                        // the ptr
                        self.internal_ptr.drop();
                        forget(self);
                    }
                    return Err(OneshotReceiveError());
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

/// [`OneshotAsyncReceiver<T>`] is the receiver side of oneshot channel that can
/// receive a single message asynchronously. It can be converted to
/// [`OneshotReceiver<T>`] by calling [`Self::to_sync`]. Receiving a message is
/// achievable with [`Self::recv`] which returns a future that should be polled
/// to receive the message.
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
        try_drop_internal(self.internal_ptr);
    }
}

#[cfg(feature = "async")]
impl<T> OneshotAsyncSender<T> {
    /// Returns a future to send a message to the channel and consumes the send
    /// side. Returning future should be polled until completion to finish
    /// the send action. Unlike other rust's one-shot channel
    /// implementations this function could be blocking/suspending only if
    /// the sender reaches the function before receiver calling its recv
    /// function. In case of successful operation this function returns
    /// `Poll::ready(Ok(()))` and in case of failure it returns back the
    /// sending object as `Poll::ready(Err(T))`.
    pub fn send(self, data: T) -> OneshotSendFuture<T> {
        let internal_ptr = self.internal_ptr;
        // No need to worry about droping the internal pointer in self, future
        // is the new owner of the pointer.
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
    /// Returns a receive future for receiving data from the channel and
    /// consumes the receive side.
    pub fn recv(self) -> OneshotReceiveFuture<T> {
        let internal_ptr = self.internal_ptr;
        // No need to worry about droping the internal pointer in self, future
        // is the new owner of the pointer.
        forget(self);
        OneshotReceiveFuture {
            state: FutureState::Zero,
            internal_ptr,
            sig: Signal::new_async(),
            data: MaybeUninit::uninit(),
            _pinned: PhantomPinned,
        }
    }
    /// Converts async receiver to sync variant of it to be used in sync context
    pub fn to_sync(self) -> OneshotReceiver<T> {
        // Safety: both structs are same with different methods
        unsafe { transmute(self) }
    }
}

/// Creates new oneshot channel and returns the [`OneshotSender<T>`] and
/// [`OneshotReceiver<T>`] for it.
///
/// # Examples
///
/// ```
/// # use std::thread::spawn;
///  let (s, r) = kanal::oneshot();
///  spawn(move || {
///        s.send("Hello").unwrap();
///       anyhow::Ok(())
///  });
///  let name=r.recv()?;
///  println!("Hello {}!",name);
/// # anyhow::Ok(())
/// ```
#[cfg_attr(
    feature = "async",
    doc = r##"
```
# tokio::runtime::Runtime::new().unwrap().block_on(async {
# use tokio::{spawn as co};
# use std::time::Duration;
  let (s, r) = kanal::oneshot();
  // launch a coroutine for tokio
  co(async move {
    // convert to async api and send message asynchronously
    s.to_async().send("World").await.unwrap();
  });
  let name=r.recv()?;
  println!("Hello {}!",name);
# anyhow::Ok(())
# });
```
"##
)]

pub fn oneshot<T>() -> (OneshotSender<T>, OneshotReceiver<T>) {
    let ptr = Box::into_raw(Box::new(OneshotInternal {
        ptr: (WAITING as *mut Signal<T>).into(),
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

/// Creates new oneshot channel and returns the async [`OneshotAsyncSender<T>`]
/// and [`OneshotAsyncReceiver<T>`] for it.
///
/// # Examples
///
/// ```
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// # use tokio::{spawn as co};
/// # use std::time::Duration;
///   let (s, r) = kanal::oneshot_async();
///   co(async move {
///     s.send("World").await.unwrap();
///   });
///   let name=r.recv().await?;
///   println!("Hello {}!",name);
/// # anyhow::Ok(())
/// # });
/// ```
///
/// ```
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// # use std::time::Duration;
///   let (s, r) = kanal::oneshot_async();
///   std::thread::spawn(move || {
///     // Convert to sync api and send with sync api.
///     s.to_sync().send("World").unwrap();
///   });
///   let name=r.recv().await?;
///   println!("Hello {}!",name);
/// # anyhow::Ok(())
/// # });
/// ```
#[cfg(feature = "async")]
pub fn oneshot_async<T>() -> (OneshotAsyncSender<T>, OneshotAsyncReceiver<T>) {
    let ptr = Box::into_raw(Box::new(OneshotInternal {
        ptr: (WAITING as *mut Signal<T>).into(),
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

/// Drops the future and returns true if future task is completed
#[inline(always)]
#[cfg(feature = "async")]
fn drop_future<T>(
    state: &FutureState,
    internal_ptr: OneshotInternalPointer<T>,
    sig: &Signal<T>,
) -> bool {
    match state {
        FutureState::Zero => {
            try_drop_internal(internal_ptr);
            false
        }
        FutureState::Waiting => {
            // If local state is waiting, this side won the signal pointer,
            // loser will never go to waiting state under no
            // condition.
            // Safety: other side can't drop internal, if this side is not in finishing
            // stage
            let internal = unsafe { internal_ptr.as_ref() };
            match internal.try_finish(sig) {
                ActionResult::Ok => {
                    // Data is not moved, as the winner itself finished the state
                    false
                }
                ActionResult::Racing | ActionResult::Winner(_) => {
                    // Signal is owned, only this side can change it to racing to give a change
                    // to other side to win the signal again, and it will not to. Winner can't
                    // be anything but this signal.
                    unreachable!(
                        "won signal can't change state to racing or winner from other side"
                    )
                }
                ActionResult::Finish => {
                    // otherside already received signal wait for the result
                    let success = sig.async_blocking_wait();
                    // Safety: winner is this side and transfer is done, this side is
                    // responsible for dropping the internal
                    unsafe {
                        internal_ptr.drop();
                    }
                    success
                }
            }
        }
        FutureState::Done => {
            // No action required, when future reaches the
            // done state, it already handled the internal_ptr drop in its
            // future execution
            true
        }
    }
}

/// Oneshot channel send future that asynchronously sends data to the oneshot
/// receiver. returns `Ok(())` and in case of failure it returns back the
/// sending object as `Err(T)`.
#[must_use = "futures do nothing unless you .await or poll them"]
#[cfg(feature = "async")]
pub struct OneshotSendFuture<T> {
    state: FutureState,
    internal_ptr: OneshotInternalPointer<T>,
    sig: Signal<T>,
    data: MaybeUninit<T>,
}

#[cfg(feature = "async")]
impl<T> Drop for OneshotSendFuture<T> {
    fn drop(&mut self) {
        if !drop_future(&self.state, self.internal_ptr, &self.sig) {
            // Send failed and this future can't return the owned send data as error so it
            // should drop it
            if needs_drop::<T>() {
                unsafe {
                    self.drop_local_data();
                }
            }
        }
    }
}

#[cfg(feature = "async")]
impl<T> Debug for OneshotSendFuture<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OneshotSendFuture {{ .. }}")
    }
}

#[cfg(feature = "async")]
impl<T> OneshotSendFuture<T> {
    /// # Safety
    /// it's only safe to call this function once and only if send operation
    /// will finish after this call.
    #[inline(always)]
    unsafe fn read_local_data(&self) -> T {
        if size_of::<T>() > size_of::<*mut T>() {
            // if its smaller than register size, it does not need pointer setup
            // as data will be stored in register address object
            ptr::read(self.data.as_ptr())
        } else {
            self.sig.assume_init()
        }
    }
    /// # Safety
    /// it's only safe to call this function once and only if send operation
    /// fails
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
        // Safety: other side can't drop internal, if this side is not in finishing
        // stage
        let internal = unsafe { this.internal_ptr.as_ref() };
        loop {
            match this.state {
                FutureState::Zero => {
                    if size_of::<T>() > size_of::<*mut T>() {
                        // if type T smaller than register size, it does not need pointer setup as
                        // data will be stored in register address object
                        this.sig
                            .set_ptr(KanalPtr::new_unchecked(this.data.as_mut_ptr()));
                    }
                    this.sig.register_waker(cx.waker());

                    match internal.try_win_race(&this.sig) {
                        ActionResult::Ok => {
                            this.state = FutureState::Waiting;
                            return Poll::Pending;
                        }
                        ActionResult::Racing => {
                            unreachable!("already tried updating from this state")
                        }
                        ActionResult::Winner(receiver) => {
                            match internal.try_finish(receiver) {
                                ActionResult::Ok => {
                                    this.state = FutureState::Done;
                                    // Other side is responsible for dropping
                                    // Safety: signal is valid and recv will be called only once as
                                    // this call will consume the self
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
                                ActionResult::Winner(_) => {
                                    unreachable!("winner can't change address to a new winner")
                                }
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
                        Poll::Ready(success) => {
                            this.state = FutureState::Done;
                            // Safety: winner is this side and transfer is done, this side is
                            // responsible for dropping the internal
                            unsafe {
                                this.internal_ptr.drop();
                            }
                            return if success {
                                Poll::Ready(Ok(()))
                            } else {
                                // Safety: data movement failed, function returns back sending data
                                // to caller
                                Poll::Ready(Err(unsafe { this.read_local_data() }))
                            };
                        }
                        Poll::Pending => {
                            if !this.sig.will_wake(cx.waker()) {
                                // the Waker is changed and we need to update waker, but we can't
                                // because it's possible to simultaneously other side pick the
                                // signal and receive mutable access to it, so we will try to reset
                                // state to racing, update waker, then trying to win the signal
                                // pointer again
                                if internal.try_reset(&this.sig).is_ok() {
                                    // Signal pointer is released, participate in the race again
                                    this.state = FutureState::Zero;
                                    continue;
                                }
                                this.state = FutureState::Done;
                                // the signal is already shared, and data will
                                // be available shortly, so wait synchronously and return the result
                                // note: it's not possible safely to update waker after the signal
                                // is shared, but we know data will be ready shortly, we can wait
                                // synchronously and receive it.
                                let success = this.sig.async_blocking_wait();
                                // Safety: winner is this side and transfer is done, this side is
                                // responsible for dropping the internal
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

/// Oneshot channel receive future that asynchronously receives data from
/// oneshot sender
#[must_use = "futures do nothing unless you .await or poll them"]
#[cfg(feature = "async")]
pub struct OneshotReceiveFuture<T> {
    state: FutureState,
    internal_ptr: OneshotInternalPointer<T>,
    sig: Signal<T>,
    data: MaybeUninit<T>,
    _pinned: PhantomPinned,
}

#[cfg(feature = "async")]
impl<T> Drop for OneshotReceiveFuture<T> {
    fn drop(&mut self) {
        if self.state != FutureState::Done
            && drop_future(&self.state, self.internal_ptr, &self.sig)
            && needs_drop::<T>()
        {
            // Otherside successfully send its data.
            // this side is responsible for dropping its data.
            unsafe {
                self.drop_local_data();
            }
        }
    }
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
    /// it's only safe to call this function once and only if send operation
    /// will finish after this call.
    #[inline(always)]
    unsafe fn read_local_data(&self) -> T {
        if size_of::<T>() > size_of::<*mut T>() {
            // if its smaller than register size, it does not need pointer setup
            // as data will be stored in register address object
            ptr::read(self.data.as_ptr())
        } else {
            self.sig.assume_init()
        }
    }
    /// # Safety
    /// it's only safe to call this function once and only if send operation
    /// fails
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
impl<T> Future for OneshotReceiveFuture<T> {
    type Output = Result<T, OneshotReceiveError>;
    #[inline(always)]
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        // Safety: we will never move data outside of this
        let this = unsafe { self.get_unchecked_mut() };
        // Safety: other side can't drop internal, if this side is not in
        // finishing stage
        let internal = unsafe { this.internal_ptr.as_ref() };
        loop {
            match this.state {
                FutureState::Zero => {
                    if size_of::<T>() > size_of::<*mut T>() {
                        // if type T smaller than register size, it does not need pointer setup as
                        // data will be stored in register address object
                        this.sig
                            .set_ptr(KanalPtr::new_unchecked(this.data.as_mut_ptr()));
                    }
                    this.sig.register_waker(cx.waker());

                    match internal.try_win_race(&this.sig) {
                        ActionResult::Ok => {
                            this.state = FutureState::Waiting;
                            return Poll::Pending;
                        }
                        ActionResult::Racing => {
                            unreachable!("already tried updating from this state")
                        }
                        ActionResult::Winner(sender) => match internal.try_finish(sender) {
                            ActionResult::Ok => {
                                this.state = FutureState::Done;
                                // Other side is responsible for dropping
                                // Safety: signal is valid and recv will be called only once as this
                                // call will consume the self
                                return unsafe {
                                    Poll::Ready(Ok(SignalTerminator::from(sender).recv()))
                                };
                            }
                            ActionResult::Racing => {
                                // Race is on again retry wining the signal
                                continue;
                            }
                            ActionResult::Winner(_) => {
                                unreachable!("winner can't change address to a new winner")
                            }
                            ActionResult::Finish => {
                                this.state = FutureState::Done;
                                // Safety: other side is dropped
                                unsafe {
                                    this.internal_ptr.drop();
                                }
                                return Poll::Ready(Err(OneshotReceiveError()));
                            }
                        },
                        ActionResult::Finish => {
                            this.state = FutureState::Done;
                            // Safety: other side is dropped
                            unsafe {
                                this.internal_ptr.drop();
                            }
                            return Poll::Ready(Err(OneshotReceiveError()));
                        }
                    }
                }
                FutureState::Waiting => {
                    let r = this.sig.poll();
                    match r {
                        Poll::Ready(success) => {
                            this.state = FutureState::Done;
                            // Safety: winner is this side and transfer is done, this side is
                            // responsible for dropping the internal
                            unsafe {
                                this.internal_ptr.drop();
                            }
                            return if success {
                                Poll::Ready(Ok(unsafe { this.read_local_data() }))
                            } else {
                                Poll::Ready(Err(OneshotReceiveError()))
                            };
                        }
                        Poll::Pending => {
                            if !this.sig.will_wake(cx.waker()) {
                                // the Waker is changed and we need to update waker, but we can't
                                // because it's possible to simultaneously other side pick the
                                // signal and receive mutable access to it, so we will try to reset
                                // state to racing, update waker, then trying to win the signal
                                // pointer again.
                                if internal.try_reset(&this.sig).is_ok() {
                                    // Signal pointer is released, participate in the race again
                                    this.state = FutureState::Zero;
                                    continue;
                                }
                                this.state = FutureState::Done;
                                // the signal is already shared, and data will be available shortly,
                                // so wait synchronously and return the result.
                                // note: it's not possible safely to update waker after the signal
                                // is shared, but we know data will be ready shortly, we can wait
                                // synchronously and receive it.
                                let success = this.sig.async_blocking_wait();
                                // Safety: winner is this side and transfer is done, this side is
                                // responsible for dropping the internal
                                unsafe {
                                    this.internal_ptr.drop();
                                }
                                if success {
                                    return Poll::Ready(Ok(unsafe { this.read_local_data() }));
                                }
                                return Poll::Ready(Err(OneshotReceiveError()));
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
