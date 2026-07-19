#![doc = include_str!("../README.md")]
#![warn(missing_docs, missing_debug_implementations)]

pub(crate) mod backoff;
pub(crate) mod internal;
// The custom mutex is compiled under loom regardless of the std-mutex
// feature so its loom model tests always run; the channel itself uses
// loom's own mutex in that configuration (see internal.rs).
#[cfg(any(not(feature = "std-mutex"), loom))]
pub(crate) mod mutex;
pub(crate) mod pointer;
pub(crate) mod primitives;

mod error;
#[cfg(feature = "async")]
mod future;
mod signal;

#[cfg(feature = "async")]
use core::mem::transmute;
use core::{
    fmt,
    mem::{size_of, MaybeUninit},
    pin::pin,
    time::Duration,
};
use std::{collections::VecDeque, time::Instant};

use branches::unlikely;
pub use error::*;
#[cfg(feature = "async")]
pub use future::*;
use internal::{acquire_internal, try_acquire_internal, Internal};
use pointer::KanalPtr;
use signal::*;

/// Sending side of the channel with sync API. It's possible to convert it to
/// async [`AsyncSender`] with `as_async`, `to_async` or `clone_async` based on
/// software requirement.
#[cfg_attr(
    feature = "async",
    doc = r##"
# Examples

```
let (sender, _r) = kanal::bounded::<u64>(0);
let sync_sender=sender.clone_async();
```
"##
)]
#[repr(C)]
pub struct Sender<T> {
    internal: Internal<T>,
}

/// Sending side of the channel with async API.  It's possible to convert it to
/// sync [`Sender`] with `as_sync`, `to_sync` or `clone_sync` based on software
/// requirement.
///
/// # Examples
///
/// ```
/// let (sender, _r) = kanal::bounded_async::<u64>(0);
/// let sync_sender = sender.clone_sync();
/// ```
#[cfg(feature = "async")]
#[repr(C)]
pub struct AsyncSender<T> {
    internal: Internal<T>,
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.internal.drop_send();
    }
}

#[cfg(feature = "async")]
impl<T> Drop for AsyncSender<T> {
    fn drop(&mut self) {
        self.internal.drop_send();
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            internal: self.internal.clone_send(),
        }
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sender {{ .. }}")
    }
}

#[cfg(feature = "async")]
impl<T> Clone for AsyncSender<T> {
    fn clone(&self) -> Self {
        Self {
            internal: self.internal.clone_send(),
        }
    }
}

#[cfg(feature = "async")]
impl<T> fmt::Debug for AsyncSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AsyncSender {{ .. }}")
    }
}

macro_rules! check_recv_closed_timeout {
    ($internal:ident,$data:ident) => {
        if unlikely($internal.recv_count == 0) {
            // Avoid wasting lock time on dropping failed send object
            drop($internal);
            return Err(SendTimeoutError::Closed($data));
        }
    };
}

macro_rules! shared_impl {
    () => {
        /// Returns whether the channel is bounded or not.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::bounded::<u64>(0);
        /// assert_eq!(s.is_bounded(), true);
        /// assert_eq!(r.is_bounded(), true);
        /// ```
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// assert_eq!(s.is_bounded(), false);
        /// assert_eq!(r.is_bounded(), false);
        /// ```
        pub fn is_bounded(&self) -> bool {
            self.internal.capacity() != usize::MAX
        }
        /// Returns length of the queue.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// assert_eq!(s.len(), 0);
        /// assert_eq!(r.len(), 0);
        /// s.send(10);
        /// assert_eq!(s.len(), 1);
        /// assert_eq!(r.len(), 1);
        /// ```
        pub fn len(&self) -> usize {
            acquire_internal(&self.internal).queue.len()
        }
        /// Returns whether the channel queue is empty or not.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// assert_eq!(s.is_empty(), true);
        /// assert_eq!(r.is_empty(), true);
        /// ```
        pub fn is_empty(&self) -> bool {
            acquire_internal(&self.internal).queue.is_empty()
        }
        /// Returns whether the channel queue is full or not
        /// full channels will block on send and recv calls
        /// it always returns true for zero sized channels.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::bounded(1);
        /// s.send("Hi!").unwrap();
        /// assert_eq!(s.is_full(), true);
        /// assert_eq!(r.is_full(), true);
        /// ```
        pub fn is_full(&self) -> bool {
            self.internal.capacity()
                == acquire_internal(&self.internal).queue.len()
        }
        /// Returns capacity of channel (not the queue)
        /// for unbounded channels, it will return usize::MAX.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::bounded::<u64>(0);
        /// assert_eq!(s.capacity(), 0);
        /// assert_eq!(r.capacity(), 0);
        /// ```
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// assert_eq!(s.capacity(), usize::MAX);
        /// assert_eq!(r.capacity(), usize::MAX);
        /// ```
        pub fn capacity(&self) -> usize {
            self.internal.capacity()
        }
        /// Returns count of alive receiver instances of the channel.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// let receiver_clone = r.clone();
        /// assert_eq!(r.receiver_count(), 2);
        /// ```
        pub fn receiver_count(&self) -> usize {
            acquire_internal(&self.internal).recv_count as usize
        }
        /// Returns count of alive sender instances of the channel.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// let sender_clone = s.clone();
        /// assert_eq!(r.sender_count(), 2);
        /// ```
        pub fn sender_count(&self) -> usize {
            acquire_internal(&self.internal).send_count as usize
        }
        /// Closes the channel completely on both sides and terminates waiting
        /// signals.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// // closes channel on both sides and has same effect as r.close();
        /// s.close().unwrap();
        /// assert_eq!(r.is_closed(), true);
        /// assert_eq!(s.is_closed(), true);
        /// ```
        pub fn close(&self) -> Result<(), CloseError> {
            let mut internal = acquire_internal(&self.internal);
            if unlikely(internal.recv_count == 0 && internal.send_count == 0) {
                return Err(CloseError());
            }
            internal.recv_count = 0;
            internal.send_count = 0;
            let cleanup = internal.detach_cleanup(true);
            drop(internal);
            cleanup.run();
            Ok(())
        }
        /// Returns whether the channel is closed on both side of send and
        /// receive or not.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// // closes channel on both sides and has same effect as r.close();
        /// s.close();
        /// assert_eq!(r.is_closed(), true);
        /// assert_eq!(s.is_closed(), true);
        /// ```
        pub fn is_closed(&self) -> bool {
            let internal = acquire_internal(&self.internal);
            internal.send_count == 0 && internal.recv_count == 0
        }
    };
}

macro_rules! shared_send_impl {
    () => {
        /// Tries sending to the channel without waiting on the waitlist. It
        /// returns `Ok(())` in case of a successful operation and
        /// `Err(SendTimeoutError::Timeout(data))` when the channel cannot
        /// accept the data right now, giving the data back to the caller, or
        /// `Err(SendTimeoutError::Closed(data))` in case that the channel is
        /// closed. Important note: this function is not lock-free as it
        /// acquires a mutex guard of the channel internal for a short time.
        ///
        /// # Examples
        ///
        /// ```
        /// # use std::thread::spawn;
        /// let (s, r) = kanal::bounded(0);
        /// let t = spawn(move || loop {
        ///     if s.try_send(1).is_ok() {
        ///         break;
        ///     }
        /// });
        /// assert_eq!(r.recv()?, 1);
        /// # t.join();
        /// # anyhow::Ok(())
        /// ```
        #[inline(always)]
        pub fn try_send(&self, data: T) -> Result<(), SendTimeoutError<T>> {
            let cap = self.internal.capacity();
            let mut internal = acquire_internal(&self.internal);
            check_recv_closed_timeout!(internal, data);
            if let Some(first) = internal.next_recv() {
                drop(internal);
                // SAFETY: it's safe to send to owned signal once
                unsafe { first.send(data) }
                return Ok(());
            }
            if cap > 0 && internal.queue.len() < cap {
                internal.queue.push_back(data);
                return Ok(());
            }
            Err(SendTimeoutError::Timeout(data))
        }

        /// Tries sending to the channel without waiting on the waitlist or
        /// for the internal mutex. It returns `Ok(())` in case of a
        /// successful operation and `Err(SendTimeoutError::Timeout(data))`
        /// when the channel cannot accept the data right now or its lock is
        /// contended, giving the data back to the caller, or
        /// `Err(SendTimeoutError::Closed(data))` in case that the channel is
        /// closed. Do not use this function unless you know exactly what you
        /// are doing.
        ///
        /// # Examples
        ///
        /// ```
        /// # use std::thread::spawn;
        /// let (s, r) = kanal::bounded(0);
        /// let t = spawn(move || loop {
        ///     if s.try_send_realtime(1).is_ok() {
        ///         break;
        ///     }
        /// });
        /// assert_eq!(r.recv()?, 1);
        /// # t.join();
        /// # anyhow::Ok(())
        /// ```
        #[inline(always)]
        pub fn try_send_realtime(
            &self,
            data: T,
        ) -> Result<(), SendTimeoutError<T>> {
            let cap = self.internal.capacity();
            if let Some(mut internal) = try_acquire_internal(&self.internal) {
                check_recv_closed_timeout!(internal, data);
                if let Some(first) = internal.next_recv() {
                    drop(internal);
                    // SAFETY: it's safe to send to owned signal once
                    unsafe { first.send(data) }
                    return Ok(());
                }
                if cap > 0 && internal.queue.len() < cap {
                    internal.queue.push_back(data);
                    return Ok(());
                }
            }
            Err(SendTimeoutError::Timeout(data))
        }

        /// Returns whether the receive side of the channel is closed or not.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// drop(r); // drop receiver and disconnect the receive side from the channel
        /// assert_eq!(s.is_disconnected(),true);
        /// # anyhow::Ok(())
        /// ```
        pub fn is_disconnected(&self) -> bool {
            acquire_internal(&self.internal).recv_count == 0
        }
    };
}

macro_rules! shared_recv_impl {
    () => {
        /// Tries receiving from the channel without waiting on the waitlist.
        /// It returns `Ok(Some(T))` in case of successful operation and
        /// `Ok(None)` for a failed one, or error in case that channel is
        /// closed. Important note: this function is not lock-free as it
        /// acquires a mutex guard of the channel internal for a short time.
        ///
        /// # Examples
        ///
        /// ```
        /// # use std::thread::spawn;
        /// # let (s, r) = kanal::bounded(0);
        /// # let t=spawn(move || {
        /// #      s.send("Buddy")?;
        /// #      anyhow::Ok(())
        /// # });
        /// loop {
        ///     if let Some(name) = r.try_recv()? {
        ///         println!("Hello {}!", name);
        ///         break;
        ///     }
        /// }
        /// # t.join();
        /// # anyhow::Ok(())
        /// ```
        #[inline(always)]
        pub fn try_recv(&self) -> Result<Option<T>, ReceiveError> {
            let cap = self.internal.capacity();
            let mut internal = acquire_internal(&self.internal);
            if unlikely(internal.recv_count == 0) {
                return Err(ReceiveError());
            }
            if cap > 0 {
                if let Some(v) = internal.queue.pop_front() {
                    // if there is a sender take its data and push it into
                    // the queue, deferring its wakeup to after unlock
                    let notify = match internal.next_send() {
                        // SAFETY: it's safe to receive from owned signal once
                        Some(p) => {
                            let (data, notify) = unsafe { p.recv_deferred() };
                            internal.queue.push_back(data);
                            notify
                        }
                        None => DeferredNotify::None,
                    };
                    drop(internal);
                    notify.notify();
                    return Ok(Some(v));
                }
            }
            if let Some(p) = internal.next_send() {
                // SAFETY: it's safe to receive from owned signal once
                drop(internal);
                return unsafe { Ok(Some(p.recv())) };
            }
            if unlikely(internal.send_count == 0) {
                return Err(ReceiveError());
            }
            Ok(None)
            // if the queue is not empty send the data
        }
        /// Tries receiving from the channel without waiting on the waitlist or
        /// waiting for channel internal lock. It returns `Ok(Some(T))` in case
        /// of successful operation and `Ok(None)` for a failed one, or error in
        /// case that channel is closed. Do not use this function unless you
        /// know exactly what you are doing.
        ///
        /// # Examples
        ///
        /// ```
        /// # use std::thread::spawn;
        /// # let (s, r) = kanal::bounded(0);
        /// # let t=spawn(move || {
        /// #      s.send("Buddy")?;
        /// #      anyhow::Ok(())
        /// # });
        /// loop {
        ///     if let Some(name) = r.try_recv_realtime()? {
        ///         println!("Hello {}!", name);
        ///         break;
        ///     }
        /// }
        /// # t.join();
        /// # anyhow::Ok(())
        /// ```
        #[inline(always)]
        pub fn try_recv_realtime(&self) -> Result<Option<T>, ReceiveError> {
            let cap = self.internal.capacity();
            if let Some(mut internal) = try_acquire_internal(&self.internal) {
                if unlikely(internal.recv_count == 0) {
                    return Err(ReceiveError());
                }
                if cap > 0 {
                    if let Some(v) = internal.queue.pop_front() {
                        // if there is a sender take its data and push it
                        // into the queue, deferring its wakeup to after
                        // unlock
                        let notify = match internal.next_send() {
                            // SAFETY: it's safe to receive from owned signal
                            // once
                            Some(p) => {
                                let (data, notify) =
                                    unsafe { p.recv_deferred() };
                                internal.queue.push_back(data);
                                notify
                            }
                            None => DeferredNotify::None,
                        };
                        drop(internal);
                        notify.notify();
                        return Ok(Some(v));
                    }
                }
                if let Some(p) = internal.next_send() {
                    // SAFETY: it's safe to receive from owned signal once
                    drop(internal);
                    return unsafe { Ok(Some(p.recv())) };
                }
                if unlikely(internal.send_count == 0) {
                    return Err(ReceiveError());
                }
            }
            Ok(None)
        }

        /// Drains all available messages from the channel into the provided
        /// vector and returns the number of received messages.
        ///
        /// The function is designed to be non-blocking, meaning it only
        /// processes messages that are readily available and returns
        /// immediately with whatever messages are present. It provides a count
        /// of received messages, which could be zero if no messages are
        /// available at the time of the call.
        ///
        /// When using this function, it’s a good idea to check if the returned
        /// count is zero to avoid busy-waiting in a loop. If blocking behavior
        /// is desired when the count is zero, you can use
        /// [`Self::drain_into_blocking`]. For efficiency, reusing the same
        /// vector across multiple calls can help minimize memory allocations.
        /// Between uses, you can clear the vector with `vec.clear()` to
        /// prepare it for the next set of messages.
        ///
        /// # Examples
        ///
        /// ```
        /// # use std::thread::spawn;
        /// # let (s, r) = kanal::bounded(1000);
        /// # let t=spawn(move || {
        /// #   for i in 0..1000 {
        /// #     s.send(i)?;
        /// #   }
        /// #   anyhow::Ok(())
        /// # });
        ///
        /// let mut buf = Vec::with_capacity(1000);
        /// loop {
        ///     if let Ok(count) = r.drain_into(&mut buf) {
        ///         if count == 0 {
        ///             // count is 0, to avoid busy-wait using recv for
        ///             // the first next message
        ///             if let Ok(v) = r.recv() {
        ///                 buf.push(v);
        ///             } else {
        ///                 break;
        ///             }
        ///         }
        ///         // use buffer
        ///         buf.iter().for_each(|v| println!("{}", v));
        ///     } else {
        ///         println!("Channel closed");
        ///         break;
        ///     }
        ///     buf.clear();
        /// }
        /// # t.join();
        /// # anyhow::Ok(())
        /// ```
        pub fn drain_into(
            &self,
            vec: &mut Vec<T>,
        ) -> Result<usize, ReceiveError> {
            let mut internal = acquire_internal(&self.internal);
            if unlikely(internal.recv_count == 0) {
                return Err(ReceiveError());
            }
            // Take the waiting senders and complete them after releasing
            // the lock, so unpark/wake side effects never run inside the
            // critical section.
            let senders = internal.take_sends();
            let required_cap = internal.queue.len() + senders.len();
            // Reserve room for every element we are about to drain;
            // Vec::reserve guarantees capacity for `len + required_cap` and
            // is a no-op when the existing capacity is already sufficient.
            vec.reserve(required_cap);
            vec.extend(internal.queue.drain(..));
            drop(internal);
            for p in senders {
                // SAFETY: it's safe to receive from owned signal once
                unsafe { vec.push(p.recv()) }
            }
            Ok(required_cap)
        }

        /// Returns, whether the send side of the channel, is closed or not.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// drop(s); // drop sender and disconnect the send side from the channel
        /// assert_eq!(r.is_disconnected(),true);
        /// ```
        pub fn is_disconnected(&self) -> bool {
            acquire_internal(&self.internal).send_count == 0
        }

        /// Returns, whether the channel receive side is terminated, and will
        /// not return any result in future recv calls.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// s.send(1).unwrap();
        /// drop(s); // drop sender and disconnect the send side from the channel
        /// assert_eq!(r.is_disconnected(),true);
        /// // Also channel is closed from send side, it's not terminated as there is data in channel queue
        /// assert_eq!(r.is_terminated(),false);
        /// assert_eq!(r.recv().unwrap(),1);
        /// // Now channel receive side is terminated as there is no sender for channel and queue is empty
        /// assert_eq!(r.is_terminated(),true);
        /// ```
        pub fn is_terminated(&self) -> bool {
            let internal = acquire_internal(&self.internal);
            internal.send_count == 0 && internal.queue.len() == 0
        }
    };
}

impl<T> Sender<T> {
    /// Sends data to the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::thread::spawn;
    /// # let (s, r) = kanal::bounded(0);
    /// # spawn(move || {
    /// s.send("Hello").unwrap();
    /// #      anyhow::Ok(())
    /// # });
    /// # let name=r.recv()?;
    /// # println!("Hello {}!",name);
    /// # anyhow::Ok(())
    /// ```
    #[inline(always)]
    pub fn send(&self, data: T) -> Result<(), SendError<T>> {
        let cap = self.internal.capacity();
        let mut internal = acquire_internal(&self.internal);
        if unlikely(internal.recv_count == 0) {
            drop(internal);
            return Err(SendError(data));
        }
        if let Some(first) = internal.next_recv() {
            drop(internal);
            // SAFETY: it's safe to send to owned signal once
            unsafe { first.send(data) }
            return Ok(());
        }
        if cap > 0 && internal.queue.len() < cap {
            // SAFETY: MaybeUninit is acting like a ManuallyDrop
            internal.queue.push_back(data);
            return Ok(());
        }
        let mut data = MaybeUninit::new(data);
        // send directly to the waitlist
        let sig = pin!(SyncSignal::new(KanalPtr::new_from(data.as_mut_ptr())));
        internal.push_signal(sig.dynamic_ptr());
        drop(internal);
        if unlikely(!sig.wait()) {
            // SAFETY: data failed to move, sender should drop it if it
            // needs to

            return Err(SendError(unsafe { data.assume_init() }));
        }
        Ok(())

        // if the queue is not empty send the data
    }

    /// Sends multiple elements from a `VecDeque` into the channel.
    ///
    /// This method attempts to push as many items from `elements` as possible,
    /// respecting the channel’s capacity and the current state of the receiver
    /// side. It behaves similarly to repeatedly calling `send` for each
    /// element, but is more efficient because it holds the internal lock
    /// only while it can make progress.
    ///
    /// * If the channel is closed (no receivers), the first element that cannot
    ///   be sent is returned inside `SendError`.
    /// * If the channel’s queue becomes full, mutex guard will be released and
    ///   remaining elements stay in the supplied `VecDeque` to be send in a
    ///   signal.
    /// * Elements are always taken from the front of the deque (FIFO order),
    ///   whether they are delivered directly to waiting receivers or moved into
    ///   the internal queue when it has spare capacity.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::collections::VecDeque;
    /// // Create a bounded channel with capacity 3
    /// let (s, r) = kanal::bounded::<i32>(3);
    ///
    /// // Move the sender and the buffer into a new thread that will
    /// // push as many items as the channel can accept.
    /// let handle = std::thread::spawn(move || {
    ///     /// // Prepare a deque with several values
    ///     let mut buf = VecDeque::from(vec![1, 2, 3, 4, 5]);
    ///     // `send_many` consumes items from the front of the deque.
    ///     // It returns `Ok(())` when all possible items have been sent
    ///     // or `Err` if the channel is closed. Here we unwrap the result
    ///     // because the channel stays alive for the whole test.
    ///     s.send_many(&mut buf).unwrap();
    ///
    ///     // Return the (now‑partially‑filled) buffer so the main thread can
    ///     // inspect the remaining elements.
    ///     buf
    /// });
    ///
    /// // In the current thread we receive the three items that fit into the
    /// // channel's capacity.
    /// assert_eq!(r.recv().unwrap(), 1);
    /// assert_eq!(r.recv().unwrap(), 2);
    /// assert_eq!(r.recv().unwrap(), 3);
    ///
    /// std::thread::sleep(std::time::Duration::from_millis(100));
    ///
    /// // Sender now written two more items into the channel queue and exited.
    /// let remaining = handle.join().expect("sender thread panicked");
    ///
    /// assert_eq!(r.len(), 2);
    /// assert_eq!(r.recv().unwrap(), 4);
    /// assert_eq!(r.recv().unwrap(), 5);
    /// ```
    ///
    /// The function returns `Ok(())` when all elements have been successfully
    /// transferred, or `Err(SendError<T>)` containing the first element that
    /// could not be sent (typically because the receiver side has been
    /// closed).
    pub fn send_many(
        &self,
        elements: &mut VecDeque<T>,
    ) -> Result<(), SendError<T>> {
        if unlikely(elements.is_empty()) {
            return Ok(());
        }
        let cap = self.internal.capacity();
        loop {
            let mut internal = acquire_internal(&self.internal);
            if unlikely(internal.recv_count == 0) {
                drop(internal);
                return Err(SendError(elements.pop_front().unwrap()));
            }
            // Take the receivers this batch can serve and complete them
            // after releasing the lock, so unpark/wake side effects never
            // run inside the critical section.
            let receivers = internal.take_recvs(elements.len());
            if !receivers.is_empty() {
                drop(internal);
                for first in receivers {
                    // SAFETY: it's safe to send to owned signal once
                    unsafe {
                        first.send(elements.pop_front().unwrap());
                    }
                }
                if unlikely(elements.is_empty()) {
                    return Ok(());
                }
                continue;
            }
            if cap > 0 {
                while internal.queue.len() < cap {
                    if let Some(v) = elements.pop_front() {
                        internal.queue.push_back(v);
                    } else {
                        return Ok(());
                    }
                }
                if unlikely(elements.is_empty()) {
                    return Ok(());
                }
            }
            let mut data = MaybeUninit::new(elements.pop_front().unwrap());
            // send directly to the waitlist; take_recvs already flipped the
            // waitlist over to the send side, pushing a send signal while
            // recv_blocking is set would corrupt the waitlist
            debug_assert!(!internal.recv_blocking);
            let sig =
                pin!(SyncSignal::new(KanalPtr::new_from(data.as_mut_ptr())));
            internal.push_signal(sig.dynamic_ptr());
            drop(internal);
            if unlikely(!sig.wait()) {
                // SAFETY: data failed to move, sender should drop it if it
                // needs to
                return Err(SendError(unsafe { data.assume_init() }));
            }
            if unlikely(elements.is_empty()) {
                return Ok(());
            }
        }
    }

    /// Sends data to the channel with a timeout. On failure the data is
    /// given back to the caller inside the returned [`SendTimeoutError`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::thread::spawn;
    /// # use std::time::Duration;
    /// # let (s, r) = kanal::bounded(0);
    /// # spawn(move || {
    /// s.send_timeout("Hello", Duration::from_millis(500)).unwrap();
    /// #      anyhow::Ok(())
    /// # });
    /// # let name=r.recv()?;
    /// # println!("Hello {}!",name);
    /// # anyhow::Ok(())
    /// ```
    #[inline(always)]
    pub fn send_timeout(
        &self,
        data: T,
        duration: Duration,
    ) -> Result<(), SendTimeoutError<T>> {
        let Some(deadline) = Instant::now().checked_add(duration) else {
            // Deadline exceeds the representable Instant range: the timeout
            // can never fire, so wait without one.
            return self
                .send(data)
                .map_err(|SendError(data)| SendTimeoutError::Closed(data));
        };
        self.send_deadline(data, deadline)
    }

    /// Sends data to the channel, waiting until the given deadline at most.
    /// Like [`Sender::send_timeout`] but takes an absolute [`Instant`]
    /// instead of a relative duration, which is useful when a single
    /// deadline spans multiple operations. On failure the data is given
    /// back to the caller inside the returned [`SendTimeoutError`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::thread::spawn;
    /// # use std::time::{Duration, Instant};
    /// # let (s, r) = kanal::bounded(0);
    /// # spawn(move || {
    /// let deadline = Instant::now() + Duration::from_millis(500);
    /// s.send_deadline("Hello", deadline).unwrap();
    /// #      anyhow::Ok(())
    /// # });
    /// # let name=r.recv()?;
    /// # println!("Hello {}!",name);
    /// # anyhow::Ok(())
    /// ```
    #[inline(always)]
    pub fn send_deadline(
        &self,
        data: T,
        deadline: Instant,
    ) -> Result<(), SendTimeoutError<T>> {
        let cap = self.internal.capacity();
        let mut internal = acquire_internal(&self.internal);
        if unlikely(internal.recv_count == 0) {
            // Avoid wasting lock time on dropping failed send object
            drop(internal);
            return Err(SendTimeoutError::Closed(data));
        }
        if let Some(first) = internal.next_recv() {
            drop(internal);
            // SAFETY: it's safe to send to owned signal once
            unsafe { first.send(data) }
            return Ok(());
        }
        if cap > 0 && internal.queue.len() < cap {
            // SAFETY: MaybeUninit is used as a ManuallyDrop, and data in it is
            // valid.
            internal.queue.push_back(data);
            return Ok(());
        }
        let mut data = MaybeUninit::new(data);
        // send directly to the waitlist
        let sig = pin!(SyncSignal::new(KanalPtr::new_from(data.as_mut_ptr())));
        internal.push_signal(sig.dynamic_ptr());
        drop(internal);
        if unlikely(!sig.wait_timeout(deadline)) {
            if sig.is_terminated() {
                // SAFETY: data failed to move, sender should drop it if it
                // needs to
                return Err(SendTimeoutError::Closed(unsafe {
                    data.assume_init()
                }));
            }
            {
                let mut internal = acquire_internal(&self.internal);
                if internal.cancel_send_signal(sig.as_tagged_ptr()) {
                    // SAFETY: data failed to move, we return it to the user
                    return Err(SendTimeoutError::Timeout(unsafe {
                        data.assume_init()
                    }));
                }
            }
            // removing receive failed to wait for the signal response
            if unlikely(!sig.wait()) {
                // SAFETY: data failed to move, we return it to the user

                return Err(SendTimeoutError::Closed(unsafe {
                    data.assume_init()
                }));
            }
        }
        Ok(())

        // if the queue is not empty send the data
    }

    shared_send_impl!();
    /// Clones [`Sender`] as the async version of it and returns it
    #[cfg(feature = "async")]
    pub fn clone_async(&self) -> AsyncSender<T> {
        AsyncSender::<T> {
            internal: self.internal.clone_send(),
        }
    }

    /// Converts [`Sender`] to [`AsyncSender`] and returns it
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use tokio::{spawn as co};
    /// # use std::time::Duration;
    /// let (s, r) = kanal::bounded(0);
    /// co(async move {
    ///     let s = s.to_async();
    ///     s.send("World").await;
    /// });
    /// let name = r.recv()?;
    /// println!("Hello {}!", name);
    /// # anyhow::Ok(())
    /// # });
    /// ```
    #[cfg(feature = "async")]
    pub fn to_async(self) -> AsyncSender<T> {
        // SAFETY: structure of Sender<T> and AsyncSender<T> is same
        unsafe { transmute(self) }
    }

    /// Borrows [`Sender`] as [`AsyncSender`] and returns it
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use tokio::{spawn as co};
    /// # use std::time::Duration;
    /// let (s, r) = kanal::bounded(0);
    /// co(async move {
    ///     s.as_async().send("World").await;
    /// });
    /// let name = r.recv()?;
    /// println!("Hello {}!", name);
    /// # anyhow::Ok(())
    /// # });
    /// ```
    #[cfg(feature = "async")]
    pub fn as_async(&self) -> &AsyncSender<T> {
        // SAFETY: structure of Sender<T> and AsyncSender<T> is same
        unsafe { transmute(self) }
    }
    shared_impl!();
}

#[cfg(feature = "async")]
impl<T> AsyncSender<T> {
    /// Sends data asynchronously to the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # let (s, r) = kanal::unbounded_async();
    /// s.send(1).await?;
    /// assert_eq!(r.recv().await?, 1);
    /// # anyhow::Ok(())
    /// # });
    /// ```
    #[inline(always)]
    pub fn send(&'_ self, data: T) -> SendFuture<'_, T> {
        SendFuture::new(&self.internal, data)
    }

    /// Sends multiple elements from a `VecDeque` into the channel
    /// asynchronously.
    ///
    /// This method consumes the provided `VecDeque` by repeatedly popping
    /// elements from its front and sending each one over the channel. The
    /// operation completes when the deque is empty or when the channel is
    /// closed.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tokio::{spawn as co};
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use std::collections::VecDeque;
    /// let (s, r) = kanal::bounded_async(3);
    /// let handle = co(async move {
    ///     let mut elems = VecDeque::from(vec![10, 20, 30, 40, 50]);
    ///     // Send all elements in the deque
    ///     s.send_many(&mut elems).await.unwrap();
    /// });
    ///
    /// // Receive the values in the same order they were sent
    /// assert_eq!(r.recv().await?, 10);
    /// assert_eq!(r.recv().await?, 20);
    /// assert_eq!(r.recv().await?, 30);
    ///
    /// //panic!("here");
    ///
    /// tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    /// // Now the sender has sent the remaining elements
    /// //handle.await.unwrap();
    ///
    /// assert_eq!(r.recv().await?, 40);
    /// assert_eq!(r.recv().await?, 50);
    ///
    /// # anyhow::Ok(())
    /// # });
    /// ```
    #[inline(always)]
    pub fn send_many<'a, 'b>(
        &'a self,
        elements: &'b mut VecDeque<T>,
    ) -> SendManyFuture<'a, 'b, T> {
        SendManyFuture::new(&self.internal, elements)
    }

    shared_send_impl!();

    /// Clones [`AsyncSender`] as [`Sender`] with sync api of it.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let (s, r) = kanal::unbounded_async();
    /// let sync_sender = s.clone_sync();
    /// // JUST FOR EXAMPLE IT IS WRONG TO USE SYNC INSTANCE IN ASYNC CONTEXT
    /// sync_sender.send(1)?;
    /// assert_eq!(r.recv().await?, 1);
    /// # anyhow::Ok(())
    /// # });
    /// ```
    pub fn clone_sync(&self) -> Sender<T> {
        Sender::<T> {
            internal: self.internal.clone_send(),
        }
    }

    /// Converts [`AsyncSender`] to [`Sender`] and returns it.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use std::time::Duration;
    /// let (s, r) = kanal::bounded_async(0);
    /// // move to sync environment
    /// std::thread::spawn(move || {
    ///     let s = s.to_sync();
    ///     s.send("World")?;
    ///     anyhow::Ok(())
    /// });
    /// let name = r.recv().await?;
    /// println!("Hello {}!", name);
    /// # anyhow::Ok(())
    /// # });
    /// ```
    pub fn to_sync(self) -> Sender<T> {
        // SAFETY: structure of Sender<T> and AsyncSender<T> is same
        unsafe { transmute(self) }
    }

    /// Borrows [`AsyncSender`] as [`Sender`] and returns it.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use std::time::Duration;
    /// let (s, r) = kanal::bounded_async(0);
    /// // move to sync environment
    /// std::thread::spawn(move || {
    ///     s.as_sync().send("World")?;
    ///     anyhow::Ok(())
    /// });
    /// let name = r.recv().await?;
    /// println!("Hello {}!", name);
    /// # anyhow::Ok(())
    /// # });
    /// ```
    pub fn as_sync(&self) -> &Sender<T> {
        // SAFETY: structure of Sender<T> and AsyncSender<T> is same
        unsafe { transmute(self) }
    }

    shared_impl!();
}

/// Receiving side of the channel in sync mode.
/// Receivers can be cloned and produce receivers to operate in both sync and
/// async modes.
#[cfg_attr(
    feature = "async",
    doc = r##"
# Examples

```
let (_s, receiver) = kanal::bounded::<u64>(0);
let async_receiver=receiver.clone_async();
```
"##
)]
#[repr(C)]
pub struct Receiver<T> {
    internal: Internal<T>,
}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Receiver {{ .. }}")
    }
}

/// [`AsyncReceiver`] is receiving side of the channel in async mode.
/// Receivers can be cloned and produce receivers to operate in both sync and
/// async modes.
///
/// # Examples
///
/// ```
/// let (_s, receiver) = kanal::bounded_async::<u64>(0);
/// let sync_receiver = receiver.clone_sync();
/// ```
#[cfg(feature = "async")]
#[repr(C)]
pub struct AsyncReceiver<T> {
    internal: Internal<T>,
}

#[cfg(feature = "async")]
impl<T> fmt::Debug for AsyncReceiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AsyncReceiver {{ .. }}")
    }
}

impl<T> Receiver<T> {
    /// Receives data from the channel
    #[inline(always)]
    pub fn recv(&self) -> Result<T, ReceiveError> {
        let cap = self.internal.capacity();
        let mut internal = acquire_internal(&self.internal);
        if unlikely(internal.recv_count == 0) {
            return Err(ReceiveError());
        }
        if cap > 0 {
            if let Some(v) = internal.queue.pop_front() {
                // if there is a sender take its data and push it into the
                // queue, deferring its wakeup to after unlock
                let notify = match internal.next_send() {
                    // SAFETY: it's safe to receive from owned signal once
                    Some(p) => {
                        let (data, notify) = unsafe { p.recv_deferred() };
                        internal.queue.push_back(data);
                        notify
                    }
                    None => DeferredNotify::None,
                };
                drop(internal);
                notify.notify();
                return Ok(v);
            }
        }
        if let Some(p) = internal.next_send() {
            drop(internal);
            // SAFETY: it's safe to receive from owned signal once
            return unsafe { Ok(p.recv()) };
        }
        if unlikely(internal.send_count == 0) {
            return Err(ReceiveError());
        }
        // no active waiter so push to the queue
        let mut ret = MaybeUninit::<T>::uninit();
        let sig = pin!(SyncSignal::new(KanalPtr::new_write_address_ptr(
            ret.as_mut_ptr()
        )));
        internal.push_signal(sig.dynamic_ptr());
        drop(internal);

        if unlikely(!sig.wait()) {
            return Err(ReceiveError());
        }

        // SAFETY: it's safe to assume init as data is forgotten on another
        // side
        if size_of::<T>() > size_of::<*mut T>() {
            Ok(unsafe { ret.assume_init() })
        } else {
            Ok(unsafe { sig.assume_init() })
        }

        // if the queue is not empty send the data
    }
    /// Tries receiving from the channel within a duration
    #[inline(always)]
    pub fn recv_timeout(
        &self,
        duration: Duration,
    ) -> Result<T, ReceiveErrorTimeout> {
        let Some(deadline) = Instant::now().checked_add(duration) else {
            // Deadline exceeds the representable Instant range: the timeout
            // can never fire, so wait without one.
            return self.recv().map_err(|_| ReceiveErrorTimeout::Closed);
        };
        self.recv_deadline(deadline)
    }

    /// Tries receiving from the channel, waiting until the given deadline at
    /// most. Like [`Receiver::recv_timeout`] but takes an absolute
    /// [`Instant`] instead of a relative duration, which is useful when a
    /// single deadline spans multiple operations.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::thread::spawn;
    /// # use std::time::{Duration, Instant};
    /// # let (s, r) = kanal::bounded(0);
    /// # let t = spawn(move || {
    /// #     s.send("Hello")?;
    /// #     anyhow::Ok(())
    /// # });
    /// let deadline = Instant::now() + Duration::from_millis(500);
    /// let name = r.recv_deadline(deadline)?;
    /// # println!("Hello {}!", name);
    /// # t.join();
    /// # anyhow::Ok(())
    /// ```
    #[inline(always)]
    pub fn recv_deadline(
        &self,
        deadline: Instant,
    ) -> Result<T, ReceiveErrorTimeout> {
        let cap = self.internal.capacity();
        let mut internal = acquire_internal(&self.internal);
        if unlikely(internal.recv_count == 0) {
            return Err(ReceiveErrorTimeout::Closed);
        }
        if cap > 0 {
            if let Some(v) = internal.queue.pop_front() {
                // if there is a sender take its data and push it into the
                // queue, deferring its wakeup to after unlock
                let notify = match internal.next_send() {
                    // SAFETY: it's safe to receive from owned signal once
                    Some(p) => {
                        let (data, notify) = unsafe { p.recv_deferred() };
                        internal.queue.push_back(data);
                        notify
                    }
                    None => DeferredNotify::None,
                };
                drop(internal);
                notify.notify();
                return Ok(v);
            }
        }
        if let Some(p) = internal.next_send() {
            drop(internal);
            // SAFETY: it's safe to receive from owned signal once
            return unsafe { Ok(p.recv()) };
        }
        if unlikely(Instant::now() > deadline) {
            return Err(ReceiveErrorTimeout::Timeout);
        }
        if unlikely(internal.send_count == 0) {
            return Err(ReceiveErrorTimeout::Closed);
        }
        // no active waiter so push to the queue
        let mut ret = MaybeUninit::<T>::uninit();
        let sig = pin!(SyncSignal::new(KanalPtr::new_write_address_ptr(
            ret.as_mut_ptr()
        )));
        internal.push_signal(sig.dynamic_ptr());
        drop(internal);
        if unlikely(!sig.wait_timeout(deadline)) {
            if sig.is_terminated() {
                return Err(ReceiveErrorTimeout::Closed);
            }
            {
                let mut internal = acquire_internal(&self.internal);
                if internal.cancel_recv_signal(sig.as_tagged_ptr()) {
                    return Err(ReceiveErrorTimeout::Timeout);
                }
            }
            // removing receive failed to wait for the signal response
            if unlikely(!sig.wait()) {
                return Err(ReceiveErrorTimeout::Closed);
            }
        }
        // SAFETY: it's safe to assume init as data is forgotten on another
        // side
        if size_of::<T>() > size_of::<*mut T>() {
            Ok(unsafe { ret.assume_init() })
        } else {
            Ok(unsafe { sig.assume_init() })
        }

        // if the queue is not empty send the data
    }

    /// Drains available messages from the channel into the provided
    /// vector, blocking the calling thread until at least one message is
    /// available, and returns the number of received messages.
    ///
    /// This is the blocking counterpart of [`Self::drain_into`]: when
    /// messages are already available it drains them all in a single
    /// batch without waiting, otherwise it waits for the first message
    /// like `recv()` and then also picks up whatever else arrived in the
    /// meantime, so the returned count is always at least 1.
    ///
    /// It returns an error if the channel is closed, or if it becomes
    /// terminated while nothing is available (all senders are dropped and
    /// the queue is empty). For efficiency, reusing the same vector
    /// across multiple calls can help minimize memory allocations.
    ///
    /// # Blocking
    ///
    /// This call blocks the calling thread; in async contexts use
    /// `AsyncReceiver::drain_into_blocking` (with the `async` feature),
    /// which returns a future instead of blocking the thread.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::thread::spawn;
    /// let (s, r) = kanal::bounded(16);
    /// let t = spawn(move || {
    ///     for i in 0..1000 {
    ///         s.send(i).unwrap();
    ///     }
    /// });
    /// let mut buf = Vec::new();
    /// while buf.len() < 1000 {
    ///     // waits for at least one message, then drains every message
    ///     // that is already available
    ///     r.drain_into_blocking(&mut buf).unwrap();
    /// }
    /// assert_eq!(buf, (0..1000).collect::<Vec<_>>());
    /// # t.join().unwrap();
    /// ```
    pub fn drain_into_blocking(
        &self,
        vec: &mut Vec<T>,
    ) -> Result<usize, ReceiveError> {
        let mut internal = acquire_internal(&self.internal);
        if unlikely(internal.recv_count == 0) {
            return Err(ReceiveError());
        }
        // Take the waiting senders and complete them after releasing
        // the lock, so unpark/wake side effects never run inside the
        // critical section.
        let senders = internal.take_sends();
        let available = internal.queue.len() + senders.len();
        if available > 0 {
            vec.reserve(available);
            vec.extend(internal.queue.drain(..));
            drop(internal);
            for p in senders {
                // SAFETY: it's safe to receive from owned signal once
                unsafe { vec.push(p.recv()) }
            }
            return Ok(available);
        }
        if unlikely(internal.send_count == 0) {
            return Err(ReceiveError());
        }
        // Nothing is available, wait for the first message like `recv`.
        // take_sends observed an empty waitlist and flipped it to the
        // receive side, so pushing a receive signal here upholds the
        // waitlist invariant.
        let mut ret = MaybeUninit::<T>::uninit();
        let sig = pin!(SyncSignal::new(KanalPtr::new_write_address_ptr(
            ret.as_mut_ptr()
        )));
        internal.push_signal(sig.dynamic_ptr());
        drop(internal);
        if unlikely(!sig.wait()) {
            return Err(ReceiveError());
        }
        // SAFETY: it's safe to assume init as data is forgotten on
        // another side
        vec.push(if size_of::<T>() > size_of::<*mut T>() {
            unsafe { ret.assume_init() }
        } else {
            unsafe { sig.assume_init() }
        });
        // Batch whatever arrived while this thread was waking up. The
        // closed checks are deliberately skipped as one message is
        // already owned; if the channel closed meanwhile, the detached
        // queue and waitlist simply yield nothing extra here.
        let mut internal = acquire_internal(&self.internal);
        let senders = internal.take_sends();
        let extra = internal.queue.len() + senders.len();
        vec.reserve(extra);
        vec.extend(internal.queue.drain(..));
        drop(internal);
        for p in senders {
            // SAFETY: it's safe to receive from owned signal once
            unsafe { vec.push(p.recv()) }
        }
        Ok(1 + extra)
    }

    shared_recv_impl!();
    #[cfg(feature = "async")]
    /// Clones receiver as the async version of it
    pub fn clone_async(&self) -> AsyncReceiver<T> {
        AsyncReceiver::<T> {
            internal: self.internal.clone_recv(),
        }
    }

    /// Converts [`Receiver`] to [`AsyncReceiver`] and returns it.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use tokio::{spawn as co};
    /// # use std::time::Duration;
    /// let (s, r) = kanal::bounded(0);
    /// co(async move {
    ///     let r = r.to_async();
    ///     let name = r.recv().await?;
    ///     println!("Hello {}!", name);
    ///     anyhow::Ok(())
    /// });
    /// s.send("World")?;
    /// # anyhow::Ok(())
    /// # });
    /// ```
    #[cfg(feature = "async")]
    pub fn to_async(self) -> AsyncReceiver<T> {
        // SAFETY: structure of Receiver<T> and AsyncReceiver<T> is same
        unsafe { transmute(self) }
    }

    /// Borrows [`Receiver`] as [`AsyncReceiver`] and returns it.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use tokio::{spawn as co};
    /// # use std::time::Duration;
    /// let (s, r) = kanal::bounded(0);
    /// co(async move {
    ///     let name = r.as_async().recv().await?;
    ///     println!("Hello {}!", name);
    ///     anyhow::Ok(())
    /// });
    /// s.send("World")?;
    /// # anyhow::Ok(())
    /// # });
    /// ```
    #[cfg(feature = "async")]
    pub fn as_async(&self) -> &AsyncReceiver<T> {
        // SAFETY: structure of Receiver<T> and AsyncReceiver<T> is same
        unsafe { transmute(self) }
    }

    shared_impl!();
}

impl<T> Iterator for Receiver<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.recv().ok()
    }
}

#[cfg(feature = "async")]
impl<T> AsyncReceiver<T> {
    /// Returns a [`ReceiveFuture`] to receive data from the channel
    /// asynchronously.
    ///
    /// # Cancellation and Polling Considerations
    ///
    /// Due to current limitations in Rust's handling of future cancellation, if
    /// a `ReceiveFuture` is dropped exactly at the time when new data is
    /// written to the channel, it may result in the loss of the received
    /// value. This behavior although memory-safe stems from the fact that
    /// Rust does not provide a built-in, correct mechanism for cancelling
    /// futures.
    ///
    /// On channels with a non-zero capacity, a value caught in this window is
    /// pushed back to the front of the channel queue instead of being lost.
    /// Because this happens after the delivery, the queue may momentarily
    /// hold one element more than `capacity()`, so `len()` can transiently
    /// exceed the capacity until the extra element is consumed. Only
    /// zero-capacity (rendezvous) channels can still lose the value in this
    /// window.
    ///
    /// Additionally, it is important to note that constructs such as
    /// `tokio::select!` are not correct to use with kanal async channels.
    /// Kanal's design does not rely on the conventional `poll` mechanism to
    /// read messages. Because of its internal optimizations, the future may
    /// complete without receiving the final poll, which prevents proper
    /// handling of the message.
    ///
    /// As a result, once the `ReceiveFuture` is polled for the first time
    /// (which registers the request to receive data), the programmer must
    /// commit to completing the polling process. This ensures that messages
    /// are correctly delivered and avoids potential race conditions associated
    /// with cancellation.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use tokio::{spawn as co};
    /// # let (s, r) = kanal::bounded_async(0);
    /// # co(async move {
    /// #      s.send("Buddy").await?;
    /// #      anyhow::Ok(())
    /// # });
    /// let name = r.recv().await?;
    /// println!("Hello {}", name);
    /// # anyhow::Ok(())
    /// # });
    /// ```
    #[inline(always)]
    pub fn recv(&'_ self) -> ReceiveFuture<'_, T> {
        ReceiveFuture::new_ref(&self.internal)
    }
    /// Creates a asynchronous stream for the channel to receive messages,
    /// [`ReceiveStream`] borrows the [`AsyncReceiver`], after dropping it,
    /// receiver will be available and usable again.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tokio::{spawn as co};
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// // import to be able to use stream.next() function
    /// use futures::stream::StreamExt;
    /// // import to be able to use stream.is_terminated() function
    /// use futures::stream::FusedStream;
    ///
    /// let (s, r) = kanal::unbounded_async();
    /// co(async move {
    ///     for i in 0..100 {
    ///         s.send(i).await.unwrap();
    ///     }
    /// });
    /// let mut stream = r.stream();
    /// assert!(!stream.is_terminated());
    /// for i in 0..100 {
    ///     assert_eq!(stream.next().await, Some(i));
    /// }
    /// // Stream will return None after it is terminated, and there is no other sender.
    /// assert_eq!(stream.next().await, None);
    /// assert!(stream.is_terminated());
    /// # });
    /// ```
    #[inline(always)]
    pub fn stream(&'_ self) -> ReceiveStream<'_, T> {
        ReceiveStream::new_borrowed(self)
    }
    /// Returns a [`DrainIntoFuture`] to drain available messages from the
    /// channel into the provided vector, waiting asynchronously until at
    /// least one message is available, and resolving to the number of
    /// received messages.
    ///
    /// This is the waiting counterpart of [`Self::drain_into`], it never
    /// blocks the thread; awaiting it suspends the task instead. If
    /// messages are already available, the future resolves immediately on
    /// the first poll with all of them drained in a single batch.
    /// Otherwise it waits like `recv` and resolves as soon as the first
    /// message is delivered, together with any other messages that arrived
    /// in the meantime, so the resolved count is always at least 1. It
    /// resolves to an error if the channel is closed, or if it becomes
    /// terminated while nothing is available.
    ///
    /// If the future is dropped after a sender already committed a message
    /// to it, that message is preserved by being pushed into the vector.
    ///
    /// # Examples
    ///
    /// ```
    /// # use tokio::{spawn as co};
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let (s, r) = kanal::bounded_async(16);
    /// co(async move {
    ///     for i in 0..100 {
    ///         s.send(i).await.unwrap();
    ///     }
    /// });
    /// let mut buf = Vec::new();
    /// while buf.len() < 100 {
    ///     // waits until at least one message is available, then drains
    ///     // every message that is already available
    ///     r.drain_into_blocking(&mut buf).await?;
    /// }
    /// assert_eq!(buf, (0..100).collect::<Vec<i32>>());
    /// # anyhow::Ok(())
    /// # });
    /// ```
    #[inline(always)]
    pub fn drain_into_blocking<'a, 'b>(
        &'a self,
        vec: &'b mut Vec<T>,
    ) -> DrainIntoFuture<'a, 'b, T> {
        DrainIntoFuture::new_ref(&self.internal, vec)
    }
    shared_recv_impl!();
    /// Returns sync cloned version of the receiver.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use tokio::{spawn as co};
    /// let (s, r) = kanal::unbounded_async();
    /// s.send(1).await?;
    /// let sync_receiver = r.clone_sync();
    /// // JUST FOR EXAMPLE IT IS WRONG TO USE SYNC INSTANCE IN ASYNC CONTEXT
    /// assert_eq!(sync_receiver.recv()?, 1);
    /// # anyhow::Ok(())
    /// # });
    /// ```
    pub fn clone_sync(&self) -> Receiver<T> {
        Receiver::<T> {
            internal: self.internal.clone_recv(),
        }
    }

    /// Converts [`AsyncReceiver`] to [`Receiver`] and returns it.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use std::time::Duration;
    /// let (s, r) = kanal::bounded_async(0);
    /// // move to sync environment
    /// std::thread::spawn(move || {
    ///     let r = r.to_sync();
    ///     let name = r.recv()?;
    ///     println!("Hello {}!", name);
    ///     anyhow::Ok(())
    /// });
    /// s.send("World").await?;
    /// # anyhow::Ok(())
    /// # });
    /// ```
    pub fn to_sync(self) -> Receiver<T> {
        // SAFETY: structure of Receiver<T> and AsyncReceiver<T> is same
        unsafe { transmute(self) }
    }

    /// Borrows [`AsyncReceiver`] as [`Receiver`] and returns it
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use std::time::Duration;
    /// let (s, r) = kanal::bounded_async(0);
    /// // move to sync environment
    /// std::thread::spawn(move || {
    ///     let name = r.as_sync().recv()?;
    ///     println!("Hello {}!", name);
    ///     anyhow::Ok(())
    /// });
    /// s.send("World").await?;
    /// # anyhow::Ok(())
    /// # });
    /// ```
    pub fn as_sync(&self) -> &Receiver<T> {
        // SAFETY: structure of Receiver<T> and AsyncReceiver<T> is same
        unsafe { transmute(self) }
    }

    shared_impl!();
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.internal.drop_recv();
    }
}

#[cfg(feature = "async")]
impl<T> Drop for AsyncReceiver<T> {
    fn drop(&mut self) {
        self.internal.drop_recv();
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        Self {
            internal: self.internal.clone_recv(),
        }
    }
}

#[cfg(feature = "async")]
impl<T> Clone for AsyncReceiver<T> {
    fn clone(&self) -> Self {
        Self {
            internal: self.internal.clone_recv(),
        }
    }
}

/// Creates a new sync bounded channel with the requested buffer size, and
/// returns [`Sender`] and [`Receiver`] of the channel for type T, you can get
/// access to async API of [`AsyncSender`] and [`AsyncReceiver`] with `to_sync`,
/// `as_async` or `clone_sync` based on your requirements, by calling them on
/// sender or receiver.
///
/// # Examples
///
/// ```
/// use std::thread::spawn;
///
/// let (s, r) = kanal::bounded(0); // for channel with zero size queue, this channel always block until successful send/recv
///
/// // spawn 8 threads, that will send 100 numbers to channel reader
/// for i in 0..8{
///     let s = s.clone();
///     spawn(move || {
///         for i in 1..100{
///             s.send(i);
///         }
///     });
/// }
/// // drop local sender so the channel send side gets closed when all of the senders finished their jobs
/// drop(s);
///
/// let first = r.recv().unwrap(); // receive first msg
/// let total: u32 = first+r.sum::<u32>(); // the receiver implements iterator so you can call sum to receive sum of rest of messages
/// assert_eq!(total, 39600);
/// ```
pub fn bounded<T>(size: usize) -> (Sender<T>, Receiver<T>) {
    let internal = Internal::new(true, size);
    (
        Sender {
            internal: internal.clone_unchecked(),
        },
        Receiver { internal },
    )
}

/// Creates a new async bounded channel with the requested buffer size, and
/// returns [`AsyncSender`] and [`AsyncReceiver`] of the channel for type T, you
/// can get access to sync API of [`Sender`] and [`Receiver`] with `to_sync`,
/// `as_async` or `clone_sync` based on your requirements, by calling them on
/// async sender or receiver.
///
/// # Examples
///
/// ```
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// use tokio::spawn as co;
///
/// let (s, r) = kanal::bounded_async(0);
///
/// co(async move {
///     s.send("hello!").await?;
///     anyhow::Ok(())
/// });
///
/// assert_eq!(r.recv().await?, "hello!");
/// anyhow::Ok(())
/// # });
/// ```
#[cfg(feature = "async")]
pub fn bounded_async<T>(size: usize) -> (AsyncSender<T>, AsyncReceiver<T>) {
    let internal = Internal::new(true, size);
    (
        AsyncSender {
            internal: internal.clone_unchecked(),
        },
        AsyncReceiver { internal },
    )
}

const UNBOUNDED_STARTING_SIZE: usize = 32;

/// Creates a new sync unbounded channel, and returns [`Sender`] and
/// [`Receiver`] of the channel for type T, you can get access to async API
/// of [`AsyncSender`] and [`AsyncReceiver`] with `to_sync`, `as_async` or
/// `clone_sync` based on your requirements, by calling them on sender or
/// receiver.
///
/// # Warning
/// This unbounded channel does not shrink its queue. As a result, if the
/// receive side is exhausted or delayed, the internal queue may grow
/// substantially. This behavior is intentional and considered as a warmup
/// phase. If such growth is undesirable, consider using a bounded channel with
/// an appropriate queue size.
///
/// # Examples
///
/// ```
/// use std::thread::spawn;
///
/// let (s, r) = kanal::unbounded(); // for channel with unbounded size queue, this channel never blocks on send
///
/// // spawn 8 threads, that will send 100 numbers to the channel reader
/// for i in 0..8{
///     let s = s.clone();
///     spawn(move || {
///         for i in 1..100{
///             s.send(i);
///         }
///     });
/// }
/// // drop local sender so the channel send side gets closed when all of the senders finished their jobs
/// drop(s);
///
/// let first = r.recv().unwrap(); // receive first msg
/// let total: u32 = first+r.sum::<u32>(); // the receiver implements iterator so you can call sum to receive sum of rest of messages
/// assert_eq!(total, 39600);
/// ```
pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
    let internal = Internal::new(false, UNBOUNDED_STARTING_SIZE);
    (
        Sender {
            internal: internal.clone_unchecked(),
        },
        Receiver { internal },
    )
}

/// Creates a new async unbounded channel, and returns [`AsyncSender`] and
/// [`AsyncReceiver`] of the channel for type T, you can get access to sync API
/// of [`Sender`] and [`Receiver`] with `to_sync`, `as_async` or `clone_sync`
/// based on your requirements, by calling them on async sender or receiver.
///
/// # Warning
/// This unbounded channel does not shrink its queue. As a result, if the
/// receive side is exhausted or delayed, the internal queue may grow
/// substantially. This behavior is intentional and considered as a warmup
/// phase. If such growth is undesirable, consider using a bounded channel with
/// an appropriate queue size.
///
/// # Examples
///
/// ```
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// use tokio::spawn as co;
///
/// let (s, r) = kanal::unbounded_async();
///
/// co(async move {
///     s.send("hello!").await?;
///     anyhow::Ok(())
/// });
///
/// assert_eq!(r.recv().await?, "hello!");
/// anyhow::Ok(())
/// # });
/// ```
#[cfg(feature = "async")]
pub fn unbounded_async<T>() -> (AsyncSender<T>, AsyncReceiver<T>) {
    let internal = Internal::new(false, UNBOUNDED_STARTING_SIZE);
    (
        AsyncSender {
            internal: internal.clone_unchecked(),
        },
        AsyncReceiver { internal },
    )
}
