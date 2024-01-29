#![doc = include_str!("../README.md")]
#![warn(missing_docs, missing_debug_implementations)]

pub(crate) mod backoff;
pub(crate) mod internal;
#[cfg(not(feature = "std-mutex"))]
pub(crate) mod mutex;
pub(crate) mod pointer;

mod error;
#[cfg(feature = "async")]
mod future;
mod oneshot;
mod signal;

pub use error::*;
#[cfg(feature = "async")]
pub use future::*;
pub use oneshot::*;

use internal::{acquire_internal, try_acquire_internal, ChannelInternal, Internal};
use pointer::KanalPtr;
use signal::*;
use std::{
    fmt,
    mem::{needs_drop, size_of, MaybeUninit},
    time::{Duration, Instant},
};
#[cfg(feature = "async")]
use std::{marker::PhantomPinned, mem::transmute};

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
/// let sync_sender=sender.clone_sync();
/// ```
#[cfg(feature = "async")]
#[repr(C)]
pub struct AsyncSender<T> {
    internal: Internal<T>,
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count > 0 {
            internal.send_count -= 1;
            if internal.send_count == 0 && internal.recv_count != 0 {
                internal.terminate_signals();
            }
        }
    }
}

#[cfg(feature = "async")]
impl<T> Drop for AsyncSender<T> {
    fn drop(&mut self) {
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count > 0 {
            internal.send_count -= 1;
            if internal.send_count == 0 && internal.recv_count != 0 {
                internal.terminate_signals();
            }
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count > 0 {
            internal.send_count += 1;
        }
        Self {
            internal: self.internal.clone(),
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
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count > 0 {
            internal.send_count += 1;
        }
        Self {
            internal: self.internal.clone(),
        }
    }
}

#[cfg(feature = "async")]
impl<T> fmt::Debug for AsyncSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AsyncSender {{ .. }}")
    }
}

macro_rules! shared_impl {
    () => {
        /// Returns whether the channel is bounded or not.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::bounded::<u64>(0);
        /// assert_eq!(s.is_bounded(),true);
        /// assert_eq!(r.is_bounded(),true);
        /// ```
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// assert_eq!(s.is_bounded(),false);
        /// assert_eq!(r.is_bounded(),false);
        /// ```
        pub fn is_bounded(&self) -> bool {
            acquire_internal(&self.internal).capacity != usize::MAX
        }
        /// Returns length of the queue.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// assert_eq!(s.len(),0);
        /// assert_eq!(r.len(),0);
        /// s.send(10);
        /// assert_eq!(s.len(),1);
        /// assert_eq!(r.len(),1);
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
        /// assert_eq!(s.is_empty(),true);
        /// assert_eq!(r.is_empty(),true);
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
        /// assert_eq!(s.is_full(),true);
        /// assert_eq!(r.is_full(),true);
        /// ```
        pub fn is_full(&self) -> bool {
            let internal = acquire_internal(&self.internal);
            internal.capacity == internal.queue.len()
        }
        /// Returns capacity of channel (not the queue)
        /// for unbounded channels, it will return usize::MAX.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::bounded::<u64>(0);
        /// assert_eq!(s.capacity(),0);
        /// assert_eq!(r.capacity(),0);
        /// ```
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// assert_eq!(s.capacity(),usize::MAX);
        /// assert_eq!(r.capacity(),usize::MAX);
        /// ```
        pub fn capacity(&self) -> usize {
            acquire_internal(&self.internal).capacity
        }
        /// Returns count of alive receiver instances of the channel.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// let receiver_clone=r.clone();
        /// assert_eq!(r.receiver_count(),2);
        /// ```
        pub fn receiver_count(&self) -> u32 {
            acquire_internal(&self.internal).recv_count
        }
        /// Returns count of alive sender instances of the channel.
        ///
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// let sender_clone=s.clone();
        /// assert_eq!(r.sender_count(),2);
        /// ```
        pub fn sender_count(&self) -> u32 {
            acquire_internal(&self.internal).send_count
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
        /// assert_eq!(r.is_closed(),true);
        /// assert_eq!(s.is_closed(),true);
        /// ```
        pub fn close(&self) -> Result<(), CloseError> {
            let mut internal = acquire_internal(&self.internal);
            if internal.recv_count == 0 && internal.send_count == 0 {
                return Err(CloseError());
            }
            internal.recv_count = 0;
            internal.send_count = 0;
            internal.terminate_signals();
            internal.queue.clear();
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
        /// assert_eq!(r.is_closed(),true);
        /// assert_eq!(s.is_closed(),true);
        /// ```
        pub fn is_closed(&self) -> bool {
            let internal = acquire_internal(&self.internal);
            internal.send_count == 0 && internal.recv_count == 0
        }
    };
}

macro_rules! shared_send_impl {
    () => {
        /// Tries sending to the channel without waiting on the waitlist, if
        /// send fails then the object will be dropped. It returns `Ok(true)` in
        /// case of a successful operation and `Ok(false)` for a failed one, or
        /// error in case that channel is closed. Important note: this function
        /// is not lock-free as it acquires a mutex guard of the channel
        /// internal for a short time.
        ///
        /// # Examples
        ///
        /// ```
        /// # use std::thread::spawn;
        /// let (s, r) = kanal::bounded(0);
        /// let t=spawn( move || {
        ///     loop{
        ///         if s.try_send(1).unwrap(){
        ///             break;
        ///         }
        ///     }
        /// });
        /// assert_eq!(r.recv()?,1);
        /// # t.join();
        /// # anyhow::Ok(())
        /// ```
        #[inline(always)]
        pub fn try_send(&self, data: T) -> Result<bool, SendError> {
            let mut internal = acquire_internal(&self.internal);
            if internal.recv_count == 0 {
                let send_count = internal.send_count;
                // Avoid wasting lock time on dropping failed send object
                drop(internal);
                if send_count == 0 {
                    return Err(SendError::Closed);
                }
                return Err(SendError::ReceiveClosed);
            }
            if let Some(first) = internal.next_recv() {
                drop(internal);
                // Safety: it's safe to send to owned signal once
                unsafe { first.send(data) }
                return Ok(true);
            } else if internal.queue.len() < internal.capacity {
                internal.queue.push_back(data);
                return Ok(true);
            }
            Ok(false)
        }

        /// Tries sending to the channel without waiting on the waitlist, if
        /// send fails then the object will be dropped. It returns `Ok(true)` in
        /// case of a successful operation and `Ok(false)` for a failed one, or
        /// error in case that channel is closed. Important note: this function
        /// is not lock-free as it acquires a mutex guard of the channel
        /// internal for a short time.
        ///
        /// # Examples
        ///
        /// ```
        /// # use std::thread::spawn;
        /// let (s, r) = kanal::bounded(0);
        /// let t=spawn( move || {
        ///     let mut opt=Some(1);
        ///     loop{
        ///         if s.try_send_option(&mut opt).unwrap(){
        ///             break;
        ///         }
        ///     }
        /// });
        /// assert_eq!(r.recv()?,1);
        /// # t.join();
        /// # anyhow::Ok(())
        /// ```
        #[inline(always)]
        pub fn try_send_option(&self, data: &mut Option<T>) -> Result<bool, SendError> {
            if data.is_none() {
                panic!("send data option is None");
            }
            let mut internal = acquire_internal(&self.internal);
            if internal.recv_count == 0 {
                let send_count = internal.send_count;
                // Avoid wasting lock time on dropping failed send object
                drop(internal);
                if send_count == 0 {
                    return Err(SendError::Closed);
                }
                return Err(SendError::ReceiveClosed);
            }
            if let Some(first) = internal.next_recv() {
                drop(internal);
                // Safety: it's safe to send to owned signal once
                unsafe { first.send(data.take().unwrap()) }
                return Ok(true);
            } else if internal.queue.len() < internal.capacity {
                internal.queue.push_back(data.take().unwrap());
                return Ok(true);
            }
            Ok(false)
        }

        /// Tries sending to the channel without waiting on the waitlist or for
        /// the internal mutex, if send fails then the object will be dropped.
        /// It returns `Ok(true)` in case of a successful operation and
        /// `Ok(false)` for a failed one, or error in case that channel is
        /// closed. Do not use this function unless you know exactly what you
        /// are doing.
        ///
        /// # Examples
        ///
        /// ```
        /// # use std::thread::spawn;
        /// let (s, r) = kanal::bounded(0);
        /// let t=spawn( move || {
        ///     loop{
        ///         if s.try_send_realtime(1).unwrap(){
        ///             break;
        ///         }
        ///     }
        /// });
        /// assert_eq!(r.recv()?,1);
        /// # t.join();
        /// # anyhow::Ok(())
        /// ```
        #[inline(always)]
        pub fn try_send_realtime(&self, data: T) -> Result<bool, SendError> {
            if let Some(mut internal) = try_acquire_internal(&self.internal) {
                if internal.recv_count == 0 {
                    let send_count = internal.send_count;
                    // Avoid wasting lock time on dropping failed send object
                    drop(internal);
                    if send_count == 0 {
                        return Err(SendError::Closed);
                    }
                    return Err(SendError::ReceiveClosed);
                }
                if let Some(first) = internal.next_recv() {
                    drop(internal);
                    // Safety: it's safe to send to owned signal once
                    unsafe { first.send(data) }
                    return Ok(true);
                } else if internal.queue.len() < internal.capacity {
                    internal.queue.push_back(data);
                    return Ok(true);
                }
            }
            Ok(false)
        }

        /// Tries sending to the channel without waiting on the waitlist or
        /// channel internal lock. It returns `Ok(true)` in case of a successful
        /// operation and `Ok(false)` for a failed one, or error in case that
        /// channel is closed. This function will `panic` on successful send
        /// attempt of `None` data. Do not use this function unless you know
        /// exactly what you are doing.
        ///
        /// # Examples
        ///
        /// ```
        /// # use std::thread::spawn;
        /// let (s, r) = kanal::bounded(0);
        /// let t=spawn( move || {
        ///     let mut opt=Some(1);
        ///     loop{
        ///         if s.try_send_option_realtime(&mut opt).unwrap(){
        ///             break;
        ///         }
        ///     }
        /// });
        /// assert_eq!(r.recv()?,1);
        /// # t.join();
        /// # anyhow::Ok(())
        /// ```
        #[inline(always)]
        pub fn try_send_option_realtime(&self, data: &mut Option<T>) -> Result<bool, SendError> {
            if data.is_none() {
                panic!("send data option is None");
            }
            if let Some(mut internal) = try_acquire_internal(&self.internal) {
                if internal.recv_count == 0 {
                    let send_count = internal.send_count;
                    // Avoid wasting lock time on dropping failed send object
                    drop(internal);
                    if send_count == 0 {
                        return Err(SendError::Closed);
                    }
                    return Err(SendError::ReceiveClosed);
                }
                if let Some(first) = internal.next_recv() {
                    drop(internal);
                    // Safety: it's safe to send to owned signal once
                    unsafe { first.send(data.take().unwrap()) }
                    return Ok(true);
                } else if internal.queue.len() < internal.capacity {
                    internal.queue.push_back(data.take().unwrap());
                    return Ok(true);
                }
            }
            Ok(false)
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
        ///     if let Some(name)=r.try_recv()?{
        ///         println!("Hello {}!",name);
        ///         break;
        ///     }
        /// }
        /// # t.join();
        /// # anyhow::Ok(())
        /// ```
        #[inline(always)]
        pub fn try_recv(&self) -> Result<Option<T>, ReceiveError> {
            let mut internal = acquire_internal(&self.internal);
            if internal.recv_count == 0 {
                return Err(ReceiveError::Closed);
            }
            if let Some(v) = internal.queue.pop_front() {
                if let Some(p) = internal.next_send() {
                    // if there is a sender take its data and push it into the
                    // queue Safety: it's safe to receive from owned
                    // signal once
                    unsafe { internal.queue.push_back(p.recv()) }
                }
                return Ok(Some(v));
            } else if let Some(p) = internal.next_send() {
                // Safety: it's safe to receive from owned signal once
                drop(internal);
                return unsafe { Ok(Some(p.recv())) };
            }
            if internal.send_count == 0 {
                return Err(ReceiveError::SendClosed);
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
        ///     if let Some(name)=r.try_recv_realtime()?{
        ///         println!("Hello {}!",name);
        ///         break;
        ///     }
        /// }
        /// # t.join();
        /// # anyhow::Ok(())
        /// ```
        #[inline(always)]
        pub fn try_recv_realtime(&self) -> Result<Option<T>, ReceiveError> {
            if let Some(mut internal) = try_acquire_internal(&self.internal) {
                if internal.recv_count == 0 {
                    return Err(ReceiveError::Closed);
                }
                if let Some(v) = internal.queue.pop_front() {
                    if let Some(p) = internal.next_send() {
                        // if there is a sender take its data and push it into
                        // the queue Safety: it's safe to
                        // receive from owned signal once
                        unsafe { internal.queue.push_back(p.recv()) }
                    }
                    return Ok(Some(v));
                } else if let Some(p) = internal.next_send() {
                    // Safety: it's safe to receive from owned signal once
                    drop(internal);
                    return unsafe { Ok(Some(p.recv())) };
                }
                if internal.send_count == 0 {
                    return Err(ReceiveError::SendClosed);
                }
            }
            Ok(None)
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
    ///  s.send("Hello").unwrap();
    /// #      anyhow::Ok(())
    /// # });
    /// # let name=r.recv()?;
    /// # println!("Hello {}!",name);
    /// # anyhow::Ok(())
    /// ```
    #[inline(always)]
    pub fn send(&self, data: T) -> Result<(), SendError> {
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count == 0 {
            let send_count = internal.send_count;
            // Avoid wasting lock time on dropping failed send object
            drop(internal);
            if send_count == 0 {
                return Err(SendError::Closed);
            }
            return Err(SendError::ReceiveClosed);
        }
        if let Some(first) = internal.next_recv() {
            drop(internal);
            // Safety: it's safe to send to owned signal once
            unsafe { first.send(data) }
            Ok(())
        } else if internal.queue.len() < internal.capacity {
            // Safety: MaybeUninit is acting like a ManuallyDrop
            internal.queue.push_back(data);
            Ok(())
        } else {
            let mut data = MaybeUninit::new(data);
            // send directly to the waitlist
            let sig = Signal::new_sync(KanalPtr::new_from(data.as_mut_ptr()));
            internal.push_send(sig.get_terminator());
            drop(internal);
            if !sig.wait() {
                // Safety: data failed to move, sender should drop it if it
                // needs to
                if needs_drop::<T>() {
                    unsafe { data.assume_init_drop() }
                }
                return Err(SendError::Closed);
            }
            Ok(())
        }
        // if the queue is not empty send the data
    }
    /// Sends data to the channel with a deadline, if send fails then the object
    /// will be dropped. you can use send_option_timeout if you like to keep
    /// the object in case of timeout.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::thread::spawn;
    /// # use std::time::Duration;
    /// # let (s, r) = kanal::bounded(0);
    /// # spawn(move || {
    ///  s.send_timeout("Hello",Duration::from_millis(500)).unwrap();
    /// #      anyhow::Ok(())
    /// # });
    /// # let name=r.recv()?;
    /// # println!("Hello {}!",name);
    /// # anyhow::Ok(())
    /// ```
    #[inline(always)]
    pub fn send_timeout(&self, data: T, duration: Duration) -> Result<(), SendErrorTimeout> {
        let deadline = Instant::now().checked_add(duration).unwrap();
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count == 0 {
            let send_count = internal.send_count;
            // Avoid wasting lock time on dropping failed send object
            drop(internal);
            if send_count == 0 {
                return Err(SendErrorTimeout::Closed);
            }
            return Err(SendErrorTimeout::ReceiveClosed);
        }
        if let Some(first) = internal.next_recv() {
            drop(internal);
            // Safety: it's safe to send to owned signal once
            unsafe { first.send(data) }
            Ok(())
        } else if internal.queue.len() < internal.capacity {
            // Safety: MaybeUninit is used as a ManuallyDrop, and data in it is
            // valid.
            internal.queue.push_back(data);
            Ok(())
        } else {
            let mut data = MaybeUninit::new(data);
            // send directly to the waitlist
            let sig = Signal::new_sync(KanalPtr::new_from(data.as_mut_ptr()));
            internal.push_send(sig.get_terminator());
            drop(internal);
            if !sig.wait_timeout(deadline) {
                if sig.is_terminated() {
                    // Safety: data failed to move, sender should drop it if it
                    // needs to
                    if needs_drop::<T>() {
                        unsafe { data.assume_init_drop() }
                    }
                    return Err(SendErrorTimeout::Closed);
                }
                {
                    let mut internal = acquire_internal(&self.internal);
                    if internal.cancel_send_signal(&sig) {
                        return Err(SendErrorTimeout::Timeout);
                    }
                }
                // removing receive failed to wait for the signal response
                if !sig.wait() {
                    // Safety: data failed to move, sender should drop it if it
                    // needs to
                    if needs_drop::<T>() {
                        unsafe { data.assume_init_drop() }
                    }
                    return Err(SendErrorTimeout::Closed);
                }
            }
            Ok(())
        }
        // if the queue is not empty send the data
    }

    /// Tries to send data from provided option with a deadline, it will panic
    /// on successful send for None option.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::thread::spawn;
    /// # use std::time::Duration;
    /// # let (s, r) = kanal::bounded(0);
    /// # spawn(move || {
    ///  let mut opt=Some("Hello");
    ///  s.send_option_timeout(&mut opt,Duration::from_millis(500)).unwrap();
    /// #      anyhow::Ok(())
    /// # });
    /// # let name=r.recv()?;
    /// # println!("Hello {}!",name);
    /// # anyhow::Ok(())
    /// ```
    #[inline(always)]
    pub fn send_option_timeout(
        &self,
        data: &mut Option<T>,
        duration: Duration,
    ) -> Result<(), SendErrorTimeout> {
        if data.is_none() {
            panic!("send data option is None");
        }
        let deadline = Instant::now().checked_add(duration).unwrap();
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count == 0 {
            let send_count = internal.send_count;
            // Avoid wasting lock time on dropping failed send object
            drop(internal);
            if send_count == 0 {
                return Err(SendErrorTimeout::Closed);
            }
            return Err(SendErrorTimeout::ReceiveClosed);
        }
        if let Some(first) = internal.next_recv() {
            drop(internal);
            // Safety: it's safe to send to owned signal once
            unsafe { first.send(data.take().unwrap()) }
            Ok(())
        } else if internal.queue.len() < internal.capacity {
            internal.queue.push_back(data.take().unwrap());
            Ok(())
        } else {
            // send directly to the waitlist
            let mut d = data.take().unwrap();
            let sig = Signal::new_sync(KanalPtr::new_from(&mut d));
            internal.push_send(sig.get_terminator());
            drop(internal);
            if !sig.wait_timeout(deadline) {
                if sig.is_terminated() {
                    *data = Some(d);
                    return Err(SendErrorTimeout::Closed);
                }
                {
                    let mut internal = acquire_internal(&self.internal);
                    if internal.cancel_send_signal(&sig) {
                        *data = Some(d);
                        return Err(SendErrorTimeout::Timeout);
                    }
                }
                // removing receive failed to wait for the signal response
                if !sig.wait() {
                    *data = Some(d);
                    return Err(SendErrorTimeout::Closed);
                }
            }
            Ok(())
        }
        // if the queue is not empty send the data
    }
    shared_send_impl!();
    /// Clones [`Sender`] as the async version of it and returns it
    #[cfg(feature = "async")]
    pub fn clone_async(&self) -> AsyncSender<T> {
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count > 0 {
            internal.send_count += 1;
        }
        AsyncSender::<T> {
            internal: self.internal.clone(),
        }
    }

    /// Converts [`Sender`] to [`AsyncSender`] and returns it
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use tokio::{spawn as co};
    /// # use std::time::Duration;
    ///   let (s, r) = kanal::bounded(0);
    ///   co(async move {
    ///     let s=s.to_async();
    ///     s.send("World").await;
    ///   });
    ///   let name=r.recv()?;
    ///   println!("Hello {}!",name);
    /// # anyhow::Ok(())
    /// # });
    /// ```
    #[cfg(feature = "async")]
    pub fn to_async(self) -> AsyncSender<T> {
        // Safety: structure of Sender<T> and AsyncSender<T> is same
        unsafe { transmute(self) }
    }

    /// Borrows [`Sender`] as [`AsyncSender`] and returns it
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use tokio::{spawn as co};
    /// # use std::time::Duration;
    ///   let (s, r) = kanal::bounded(0);
    ///   co(async move {
    ///     s.as_async().send("World").await;
    ///   });
    ///   let name=r.recv()?;
    ///   println!("Hello {}!",name);
    /// # anyhow::Ok(())
    /// # });
    /// ```
    #[cfg(feature = "async")]
    pub fn as_async(&self) -> &AsyncSender<T> {
        // Safety: structure of Sender<T> and AsyncSender<T> is same
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
    /// assert_eq!(r.recv().await?,1);
    /// # anyhow::Ok(())
    /// # });
    /// ```
    #[inline(always)]
    pub fn send(&'_ self, data: T) -> SendFuture<'_, T> {
        if size_of::<T>() > size_of::<*mut T>() {
            SendFuture {
                state: FutureState::Zero,
                internal: &self.internal,
                sig: Signal::new_async(),
                data: MaybeUninit::new(data),
                _pinned: PhantomPinned,
            }
        } else {
            SendFuture {
                state: FutureState::Zero,
                internal: &self.internal,
                sig: Signal::new_async_ptr(KanalPtr::new_owned(data)),
                data: MaybeUninit::uninit(),
                _pinned: PhantomPinned,
            }
        }
    }
    shared_send_impl!();
    /// Clones [`AsyncSender`] as [`Sender`] with sync api of it.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// let (s, r) = kanal::unbounded_async();
    /// let sync_sender=s.clone_sync();
    /// // JUST FOR EXAMPLE IT IS WRONG TO USE SYNC INSTANCE IN ASYNC CONTEXT
    /// sync_sender.send(1)?;
    /// assert_eq!(r.recv().await?,1);
    /// # anyhow::Ok(())
    /// # });
    /// ```
    pub fn clone_sync(&self) -> Sender<T> {
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count > 0 {
            internal.send_count += 1;
        }
        Sender::<T> {
            internal: self.internal.clone(),
        }
    }

    /// Converts [`AsyncSender`] to [`Sender`] and returns it.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use std::time::Duration;
    ///   let (s, r) = kanal::bounded_async(0);
    ///   // move to sync environment
    ///   std::thread::spawn(move || {
    ///     let s=s.to_sync();
    ///     s.send("World")?;
    ///     anyhow::Ok(())
    ///   });
    ///   let name=r.recv().await?;
    ///   println!("Hello {}!",name);
    /// # anyhow::Ok(())
    /// # });
    /// ```
    pub fn to_sync(self) -> Sender<T> {
        // Safety: structure of Sender<T> and AsyncSender<T> is same
        unsafe { transmute(self) }
    }

    /// Borrows [`AsyncSender`] as [`Sender`] and returns it.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use std::time::Duration;
    ///   let (s, r) = kanal::bounded_async(0);
    ///   // move to sync environment
    ///   std::thread::spawn(move || {
    ///     s.as_sync().send("World")?;
    ///     anyhow::Ok(())
    ///   });
    ///   let name=r.recv().await?;
    ///   println!("Hello {}!",name);
    /// # anyhow::Ok(())
    /// # });
    /// ```
    pub fn as_sync(&self) -> &Sender<T> {
        // Safety: structure of Sender<T> and AsyncSender<T> is same
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
/// let sync_receiver=receiver.clone_sync();
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
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count == 0 {
            return Err(ReceiveError::Closed);
        }
        if let Some(v) = internal.queue.pop_front() {
            if let Some(p) = internal.next_send() {
                // if there is a sender take its data and push it into the queue
                // Safety: it's safe to receive from owned signal once
                unsafe { internal.queue.push_back(p.recv()) }
            }
            Ok(v)
        } else if let Some(p) = internal.next_send() {
            drop(internal);
            // Safety: it's safe to receive from owned signal once
            unsafe { Ok(p.recv()) }
        } else {
            if internal.send_count == 0 {
                return Err(ReceiveError::SendClosed);
            }
            // no active waiter so push to the queue
            let mut ret = MaybeUninit::<T>::uninit();
            let sig = Signal::new_sync(KanalPtr::new_write_address_ptr(ret.as_mut_ptr()));
            internal.push_recv(sig.get_terminator());
            drop(internal);

            if !sig.wait() {
                return Err(ReceiveError::Closed);
            }

            // Safety: it's safe to assume init as data is forgotten on another
            // side
            if size_of::<T>() > size_of::<*mut T>() {
                Ok(unsafe { ret.assume_init() })
            } else {
                Ok(unsafe { sig.assume_init() })
            }
        }
        // if the queue is not empty send the data
    }
    /// Tries receiving from the channel within a duration
    #[inline(always)]
    pub fn recv_timeout(&self, duration: Duration) -> Result<T, ReceiveErrorTimeout> {
        let deadline = Instant::now().checked_add(duration).unwrap();
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count == 0 {
            return Err(ReceiveErrorTimeout::Closed);
        }
        if let Some(v) = internal.queue.pop_front() {
            if let Some(p) = internal.next_send() {
                // if there is a sender take its data and push it into the queue
                // Safety: it's safe to receive from owned signal once
                unsafe { internal.queue.push_back(p.recv()) }
            }
            Ok(v)
        } else if let Some(p) = internal.next_send() {
            drop(internal);
            // Safety: it's safe to receive from owned signal once
            unsafe { Ok(p.recv()) }
        } else {
            if Instant::now() > deadline {
                return Err(ReceiveErrorTimeout::Timeout);
            }
            if internal.send_count == 0 {
                return Err(ReceiveErrorTimeout::SendClosed);
            }
            // no active waiter so push to the queue
            let mut ret = MaybeUninit::<T>::uninit();
            let sig = Signal::new_sync(KanalPtr::new_write_address_ptr(ret.as_mut_ptr()));
            internal.push_recv(sig.get_terminator());
            drop(internal);
            if !sig.wait_timeout(deadline) {
                if sig.is_terminated() {
                    return Err(ReceiveErrorTimeout::Closed);
                }
                {
                    let mut internal = acquire_internal(&self.internal);
                    if internal.cancel_recv_signal(&sig) {
                        return Err(ReceiveErrorTimeout::Timeout);
                    }
                }
                // removing receive failed to wait for the signal response
                if !sig.wait() {
                    return Err(ReceiveErrorTimeout::Closed);
                }
            }
            // Safety: it's safe to assume init as data is forgotten on another
            // side
            if size_of::<T>() > size_of::<*mut T>() {
                Ok(unsafe { ret.assume_init() })
            } else {
                Ok(unsafe { sig.assume_init() })
            }
        }
        // if the queue is not empty send the data
    }
    shared_recv_impl!();
    #[cfg(feature = "async")]
    /// Clones receiver as the async version of it
    pub fn clone_async(&self) -> AsyncReceiver<T> {
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count > 0 {
            internal.recv_count += 1;
        }
        AsyncReceiver::<T> {
            internal: self.internal.clone(),
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
    ///   let (s, r) = kanal::bounded(0);
    ///   co(async move {
    ///     let r=r.to_async();
    ///     let name=r.recv().await?;
    ///     println!("Hello {}!",name);
    ///     anyhow::Ok(())
    ///   });
    ///   s.send("World")?;
    /// # anyhow::Ok(())
    /// # });
    /// ```
    #[cfg(feature = "async")]
    pub fn to_async(self) -> AsyncReceiver<T> {
        // Safety: structure of Receiver<T> and AsyncReceiver<T> is same
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
    ///   let (s, r) = kanal::bounded(0);
    ///   co(async move {
    ///     let name=r.as_async().recv().await?;
    ///     println!("Hello {}!",name);
    ///     anyhow::Ok(())
    ///   });
    ///   s.send("World")?;
    /// # anyhow::Ok(())
    /// # });
    /// ```
    #[cfg(feature = "async")]
    pub fn as_async(&self) -> &AsyncReceiver<T> {
        // Safety: structure of Receiver<T> and AsyncReceiver<T> is same
        unsafe { transmute(self) }
    }

    shared_impl!();
}

impl<T> Iterator for Receiver<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        match self.recv() {
            Ok(d) => Some(d),
            Err(_) => None,
        }
    }
}

#[cfg(feature = "async")]
impl<T> AsyncReceiver<T> {
    /// Returns a [`ReceiveFuture`] to receive data from the channel
    /// asynchronously.
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
    /// let name=r.recv().await?;
    /// println!("Hello {}",name);
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
    /// let sync_receiver=r.clone_sync();
    /// // JUST FOR EXAMPLE IT IS WRONG TO USE SYNC INSTANCE IN ASYNC CONTEXT
    /// assert_eq!(sync_receiver.recv()?,1);
    /// # anyhow::Ok(())
    /// # });
    /// ```
    pub fn clone_sync(&self) -> Receiver<T> {
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count > 0 {
            internal.recv_count += 1;
        }
        Receiver::<T> {
            internal: self.internal.clone(),
        }
    }

    /// Converts [`AsyncReceiver`] to [`Receiver`] and returns it.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use std::time::Duration;
    ///   let (s, r) = kanal::bounded_async(0);
    ///   // move to sync environment
    ///   std::thread::spawn(move || {
    ///     let r=r.to_sync();
    ///     let name=r.recv()?;
    ///     println!("Hello {}!",name);
    ///     anyhow::Ok(())
    ///   });
    ///   s.send("World").await?;
    /// # anyhow::Ok(())
    /// # });
    /// ```
    pub fn to_sync(self) -> Receiver<T> {
        // Safety: structure of Receiver<T> and AsyncReceiver<T> is same
        unsafe { transmute(self) }
    }

    /// Borrows [`AsyncReceiver`] as [`Receiver`] and returns it
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # use std::time::Duration;
    ///   let (s, r) = kanal::bounded_async(0);
    ///   // move to sync environment
    ///   std::thread::spawn(move || {
    ///     let name=r.as_sync().recv()?;
    ///     println!("Hello {}!",name);
    ///     anyhow::Ok(())
    ///   });
    ///   s.send("World").await?;
    /// # anyhow::Ok(())
    /// # });
    /// ```
    pub fn as_sync(&self) -> &Receiver<T> {
        // Safety: structure of Receiver<T> and AsyncReceiver<T> is same
        unsafe { transmute(self) }
    }

    shared_impl!();
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count > 0 {
            internal.recv_count -= 1;
            if internal.recv_count == 0 && internal.send_count != 0 {
                internal.terminate_signals();
            }
        }
    }
}

#[cfg(feature = "async")]
impl<T> Drop for AsyncReceiver<T> {
    fn drop(&mut self) {
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count > 0 {
            internal.recv_count -= 1;
            if internal.recv_count == 0 && internal.send_count != 0 {
                internal.terminate_signals();
            }
        }
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count > 0 {
            internal.recv_count += 1;
        }
        Self {
            internal: self.internal.clone(),
        }
    }
}

#[cfg(feature = "async")]
impl<T> Clone for AsyncReceiver<T> {
    fn clone(&self) -> Self {
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count > 0 {
            internal.recv_count += 1;
        }
        Self {
            internal: self.internal.clone(),
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
    let internal = ChannelInternal::new(true, size);
    (
        Sender {
            internal: internal.clone(),
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
/// use tokio::{spawn as co};
///
/// let (s, r) = kanal::bounded_async(0);
///
/// co(async move {
///       s.send("hello!").await?;
///       anyhow::Ok(())
/// });
///
/// assert_eq!(r.recv().await?, "hello!");
/// anyhow::Ok(())
/// # });
/// ```
#[cfg(feature = "async")]
pub fn bounded_async<T>(size: usize) -> (AsyncSender<T>, AsyncReceiver<T>) {
    let internal = ChannelInternal::new(true, size);
    (
        AsyncSender {
            internal: internal.clone(),
        },
        AsyncReceiver { internal },
    )
}

const UNBOUNDED_STARTING_SIZE: usize = 2048;

/// Creates a new sync unbounded channel, and returns [`Sender`] and
/// [`Receiver`] of the channel for type T, you can get access to async API
/// of [`AsyncSender`] and [`AsyncReceiver`] with `to_sync`, `as_async` or
/// `clone_sync` based on your requirements, by calling them on sender or
/// receiver.
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
    let internal = ChannelInternal::new(false, UNBOUNDED_STARTING_SIZE);
    (
        Sender {
            internal: internal.clone(),
        },
        Receiver { internal },
    )
}

/// Creates a new async unbounded channel, and returns [`AsyncSender`] and
/// [`AsyncReceiver`] of the channel for type T, you can get access to sync API
/// of [`Sender`] and [`Receiver`] with `to_sync`, `as_async` or `clone_sync`
/// based on your requirements, by calling them on async sender or receiver.
///
/// # Examples
///
/// ```
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// use tokio::{spawn as co};
///
/// let (s, r) = kanal::unbounded_async();
///
/// co(async move {
///       s.send("hello!").await?;
///       anyhow::Ok(())
/// });
///
/// assert_eq!(r.recv().await?, "hello!");
/// anyhow::Ok(())
/// # });
/// ```
#[cfg(feature = "async")]
pub fn unbounded_async<T>() -> (AsyncSender<T>, AsyncReceiver<T>) {
    let internal = ChannelInternal::new(false, UNBOUNDED_STARTING_SIZE);
    (
        AsyncSender {
            internal: internal.clone(),
        },
        AsyncReceiver { internal },
    )
}
