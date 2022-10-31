//! # Kanal: The fast synchronous and asynchronous channel that Rust deserves.
//!
//! Kanal is a Rust library to help programmers design effective programs in CSP model via providing featureful multi-producer multi-consumer channels.
//! This library focuses on bringing both sync and async API together to unify message passing between sync and async parts of Rust code in a performant manner.
//! Performance is the main goal of Kanal.
//!
//!
#![warn(missing_docs, missing_debug_implementations)]
#[cfg(feature = "async")]
mod future;
#[cfg(feature = "async")]
pub use future::*;

mod error;
pub use error::*;

pub(crate) mod internal;
mod kanal_tests;
pub(crate) mod mutex;
mod signal;
pub(crate) mod state;

use internal::{acquire_internal, try_acquire_internal, ChannelInternal, Internal};

use std::mem::MaybeUninit;
use std::time::{Duration, Instant};

use std::fmt;
use std::fmt::Debug;

#[cfg(feature = "async")]
use signal::AsyncSignal;
use signal::SyncSignal;

/// Sending side of the channel in sync mode.
/// Senders can be cloned and produce senders to operate in both sync and async modes.
/// # Examples
///
/// ```
/// let (sender, _r) = kanal::bounded::<u64>(0);
/// let sync_sender=sender.clone_async();
/// ```
pub struct Sender<T> {
    internal: Internal<T>,
}

/// Sending side of the channel in async mode.
/// Senders can be cloned and produce senders to operate in both sync and async modes.
/// # Examples
///
/// ```
/// let (sender, _r) = kanal::bounded_async::<u64>(0);
/// let sync_sender=sender.clone_sync();
/// ```
#[cfg(feature = "async")]
pub struct AsyncSender<T> {
    internal: Internal<T>,
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count > 0 {
            internal.send_count -= 1;
            if internal.send_count == 0 {
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
            if internal.send_count == 0 {
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

impl<T> Debug for Sender<T> {
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

impl<T> Debug for AsyncSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AsyncSender {{ .. }}")
    }
}

macro_rules! shared_impl {
    () => {
        /// Returns whether the channel is bounded or not
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
        /// Returns length of the queue
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
        /// Returns whether the channel queue is empty or not
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
        /// Returns capacity of channel (not the queue)
        /// for unbounded channels, it will return usize::MAX
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
        /// Returns count of alive receiver instances of the channel
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
        /// Returns count of alive sender instances of the channel
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
        /// Closes the channel completely on both sides and terminates waiting signals
        /// # Examples
        ///
        /// ```
        /// let (s, r) = kanal::unbounded::<u64>();
        /// // closes channel on both sides and has same effect as r.close();
        /// s.close();
        /// assert_eq!(r.is_closed(),true);
        /// assert_eq!(s.is_closed(),true);
        /// ```
        pub fn close(&self) -> bool {
            let mut internal = acquire_internal(&self.internal);
            if internal.recv_count == 0 && internal.send_count == 0 {
                return false;
            }
            internal.recv_count = 0;
            internal.send_count = 0;
            internal.terminate_signals();
            internal.send_wait.clear();
            internal.recv_wait.clear();
            true
        }
        /// Returns whether the channel is closed on both side of send and receive or not
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
        /// Tries sending to the channel without waiting on the waitlist, if send fails then the object will be dropped.
        /// It returns `Ok(true)` in case of a successful operation and `Ok(false)` for a failed one, or error in case that channel is closed.
        /// Important note: this function is not lock-free as it acquires a mutex guard of the channel internal for a short time.
        /// # Examples
        ///
        /// ```
        /// # use std::thread::spawn;
        /// let (s, r) = kanal::bounded(0);
        /// spawn( move || {
        ///     loop{
        ///         if s.try_send(1).unwrap(){
        ///             break;
        ///         }
        ///     }
        /// });
        /// assert_eq!(r.recv()?,1);
        /// # anyhow::Ok(())
        /// ```
        #[inline(always)]
        pub fn try_send(&self, data: T) -> Result<bool, SendError> {
            let mut internal = acquire_internal(&self.internal);
            if internal.send_count == 0 {
                return Err(SendError::Closed);
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
            if internal.recv_count == 0 {
                return Err(SendError::ReceiveClosed);
            }
            Ok(false)
        }

        /// Tries sending to the channel without waiting on the waitlist, if send fails then the object will be dropped.
        /// It returns `Ok(true)` in case of a successful operation and `Ok(false)` for a failed one, or error in case that channel is closed.
        /// Important note: this function is not lock-free as it acquires a mutex guard of the channel internal for a short time.\
        /// # Examples
        ///
        /// ```
        /// # use std::thread::spawn;
        /// let (s, r) = kanal::bounded(0);
        /// spawn( move || {
        ///     let mut opt=Some(1);
        ///     loop{
        ///         if s.try_send_option(&mut opt).unwrap(){
        ///             break;
        ///         }
        ///     }
        /// });
        /// assert_eq!(r.recv()?,1);
        /// # anyhow::Ok(())
        /// ```
        #[inline(always)]
        pub fn try_send_option(&self, data: &mut Option<T>) -> Result<bool, SendError> {
            let mut internal = acquire_internal(&self.internal);
            if internal.send_count == 0 {
                return Err(SendError::Closed);
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
            if internal.recv_count == 0 {
                return Err(SendError::ReceiveClosed);
            }
            Ok(false)
        }

        /// Tries sending to the channel without waiting on the waitlist or for the internal mutex, if send fails then the object will be dropped.
        /// It returns `Ok(true)` in case of a successful operation and `Ok(false)` for a failed one, or error in case that channel is closed.
        /// Do not use this function unless you know exactly what you are doing.
        /// # Examples
        ///
        /// ```
        /// # use std::thread::spawn;
        /// let (s, r) = kanal::bounded(0);
        /// spawn( move || {
        ///     loop{
        ///         if s.try_send_realtime(1).unwrap(){
        ///             break;
        ///         }
        ///     }
        /// });
        /// assert_eq!(r.recv()?,1);
        /// # anyhow::Ok(())
        /// ```
        #[inline(always)]
        pub fn try_send_realtime(&self, data: T) -> Result<bool, SendError> {
            if let Some(mut internal) = try_acquire_internal(&self.internal) {
                if internal.send_count == 0 {
                    return Err(SendError::Closed);
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
                if internal.recv_count == 0 {
                    return Err(SendError::ReceiveClosed);
                }
            }
            Ok(false)
        }

        /// Tries sending to the channel without waiting on the waitlist or channel internal lock.
        /// It returns `Ok(true)` in case of a successful operation and `Ok(false)` for a failed one, or error in case that channel is closed.
        /// This function will `panic` on successfull send attempt of `None` data.
        /// Do not use this function unless you know exactly what you are doing.
        /// # Examples
        ///
        /// ```
        /// # use std::thread::spawn;
        /// let (s, r) = kanal::bounded(0);
        /// spawn( move || {
        ///     let mut opt=Some(1);
        ///     loop{
        ///         if s.try_send_option_realtime(&mut opt).unwrap(){
        ///             break;
        ///         }
        ///     }
        /// });
        /// assert_eq!(r.recv()?,1);
        /// # anyhow::Ok(())
        /// ```
        #[inline(always)]
        pub fn try_send_option_realtime(&self, data: &mut Option<T>) -> Result<bool, SendError> {
            if let Some(mut internal) = try_acquire_internal(&self.internal) {
                if internal.send_count == 0 {
                    return Err(SendError::Closed);
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
                if internal.recv_count == 0 {
                    return Err(SendError::ReceiveClosed);
                }
            }
            Ok(false)
        }

        /// Returns whether the receive side of the channel is closed or not
        /// # Examples
        ///
        /// ```
        /// # use tokio::{spawn as co};
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
        /// It returns `Ok(Some(T))` in case of successful operation and `Ok(None)` for a failed one, or error in case that channel is closed.
        /// Important note: this function is not lock-free as it acquires a mutex guard of the channel internal for a short time.
        /// # Examples
        ///
        /// ```
        /// # use std::thread::spawn;
        /// # let (s, r) = kanal::bounded(0);
        /// # spawn(move || {
        /// #      s.send("Buddy")?;
        /// #      anyhow::Ok(())
        /// # });
        /// loop {
        ///     if let Some(name)=r.try_recv()?{
        ///         println!("Hello {}!",name);
        ///         break;
        ///     }
        /// }
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
                    // if there is a sender take its data and push it into the queue
                    // Safety: it's safe to receive from owned signal once
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
        /// Tries receiving from the channel without waiting on the waitlist or waiting for channel internal lock.
        /// It returns `Ok(Some(T))` in case of successful operation and `Ok(None)` for a failed one, or error in case that channel is closed.
        /// Do not use this function unless you know exactly what you are doing.
        /// # Examples
        ///
        /// ```
        /// # use std::thread::spawn;
        /// # let (s, r) = kanal::bounded(0);
        /// # spawn(move || {
        /// #      s.send("Buddy")?;
        /// #      anyhow::Ok(())
        /// # });
        /// loop {
        ///     if let Some(name)=r.try_recv_realtime()?{
        ///         println!("Hello {}!",name);
        ///         break;
        ///     }
        /// }
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
                        // if there is a sender take its data and push it into the queue
                        // Safety: it's safe to receive from owned signal once
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

        /// Returns, whether the send side of the channel, is closed or not
        /// # Examples
        ///
        /// ```
        /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
        /// # use tokio::{spawn as co};
        /// let (s, r) = kanal::unbounded_async::<u64>();
        /// drop(s); // drop sender and disconnect the send side from the channel
        /// assert_eq!(r.is_disconnected(),true);
        /// # anyhow::Ok(())
        /// # });
        /// ```
        pub fn is_disconnected(&self) -> bool {
            acquire_internal(&self.internal).send_count == 0
        }
    };
}

impl<T> Sender<T> {
    /// Sends data to the channel
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
    pub fn send(&self, mut data: T) -> Result<(), SendError> {
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count == 0 {
            return Err(SendError::Closed);
        }
        if let Some(first) = internal.next_recv() {
            drop(internal);
            // Safety: it's safe to send to owned signal once
            unsafe { first.send(data) }
            Ok(())
        } else if internal.queue.len() < internal.capacity {
            internal.queue.push_back(data);
            Ok(())
        } else {
            if internal.recv_count == 0 {
                return Err(SendError::ReceiveClosed);
            }
            // send directly to the waitlist
            {
                let _data_address_holder = &data; // pin to address
                let sig = SyncSignal::new(&mut data as *mut T, std::thread::current());
                let _sig_address_holder = &sig;
                internal.push_send(sig.as_signal());
                drop(internal);
                if !sig.wait() {
                    return Err(SendError::Closed);
                }
                // data semantically is moved so forget about dropping it if it requires dropping
                if std::mem::needs_drop::<T>() {
                    std::mem::forget(data);
                }
            }

            Ok(())
        }
        // if the queue is not empty send the data
    }
    /// Sends data to the channel with a deadline, if send fails then the object will be dropped.
    /// you can use send_option_timeout if you like to keep the object in case of timeout.
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
    pub fn send_timeout(&self, mut data: T, duration: Duration) -> Result<(), SendErrorTimeout> {
        let deadline = Instant::now().checked_add(duration).unwrap();
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count == 0 {
            return Err(SendErrorTimeout::Closed);
        }
        if let Some(first) = internal.next_recv() {
            drop(internal);
            // Safety: it's safe to send to owned signal once
            unsafe { first.send(data) }
            Ok(())
        } else if internal.queue.len() < internal.capacity {
            internal.queue.push_back(data);
            Ok(())
        } else {
            if internal.recv_count == 0 {
                return Err(SendErrorTimeout::ReceiveClosed);
            }
            // send directly to the waitlist
            let _data_address_holder = &data; // pin to address
            let sig = SyncSignal::new(&mut data as *mut T, std::thread::current());
            let _sig_address_holder = &sig;
            internal.push_send(sig.as_signal());
            drop(internal);
            if !sig.wait_timeout(deadline) {
                if sig.is_terminated() {
                    return Err(SendErrorTimeout::Closed);
                }
                {
                    let mut internal = acquire_internal(&self.internal);
                    if internal.cancel_send_signal(sig.as_signal()) {
                        return Err(SendErrorTimeout::Timeout);
                    }
                }
                // removing receive failed to wait for the signal response
                if !sig.wait() {
                    return Err(SendErrorTimeout::Closed);
                }
            }
            // data semantically is moved so forget about dropping it if it requires dropping
            if std::mem::needs_drop::<T>() {
                std::mem::forget(data);
            }
            Ok(())
        }
        // if the queue is not empty send the data
    }

    /// Tries to send data from provided option with a deadline, it will panic on successful send for None option.
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
        let deadline = Instant::now().checked_add(duration).unwrap();
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count == 0 {
            return Err(SendErrorTimeout::Closed);
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
            if internal.recv_count == 0 {
                return Err(SendErrorTimeout::ReceiveClosed);
            }
            // send directly to the waitlist
            let mut d = data.take().unwrap();
            let _data_address_holder = &d; // pin to address
            let sig = SyncSignal::new(&mut d as *mut T, std::thread::current());
            let _sig_address_holder = &sig;
            internal.push_send(sig.as_signal());
            drop(internal);
            if !sig.wait_timeout(deadline) {
                if sig.is_terminated() {
                    *data = Some(d);
                    return Err(SendErrorTimeout::Closed);
                }
                {
                    let mut internal = acquire_internal(&self.internal);
                    if internal.cancel_send_signal(sig.as_signal()) {
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
    /// Clones Sender as the async version of it and returns it
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
    shared_impl!();
}

#[cfg(feature = "async")]
impl<T> AsyncSender<T> {
    /// Sends data asynchronously to the channel
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
        SendFuture {
            state: FutureState::Zero,
            internal: &self.internal,
            sig: AsyncSignal::new(),
            data: MaybeUninit::new(data),
        }
    }
    shared_send_impl!();
    /// Clones async sender as sync version of it
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

    shared_impl!();
}

/// Receiving side of the channel in sync mode.
/// Receivers can be cloned and produce receivers to operate in both sync and async modes.
/// ```
/// let (_s, receiver) = kanal::bounded::<u64>(0);
/// let async_receiver=receiver.clone_async();
/// ```
pub struct Receiver<T> {
    internal: Internal<T>,
}

impl<T> Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Receiver {{ .. }}")
    }
}

/// Receiving side of the channel in async mode.
/// Receivers can be cloned and produce receivers to operate in both sync and async modes.
/// ```
/// let (_s, receiver) = kanal::bounded_async::<u64>(0);
/// let sync_receiver=receiver.clone_sync();
/// ```
#[cfg(feature = "async")]
pub struct AsyncReceiver<T> {
    internal: Internal<T>,
}

impl<T> Debug for AsyncReceiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AsyncReceiver {{ .. }}")
    }
}

impl<T> Receiver<T> {
    /// Receives data from the channel
    #[inline(always)]
    pub fn recv(&self) -> Result<T, SendError> {
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count == 0 {
            return Err(SendError::Closed);
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
                return Err(SendError::Closed);
            }
            // no active waiter so push to the queue
            let mut ret = MaybeUninit::<T>::uninit();
            let _ret_address_holder = &ret;
            {
                let sig = SyncSignal::new(ret.as_mut_ptr(), std::thread::current());
                let _sig_address_holder = &sig;
                internal.push_recv(sig.as_signal());
                drop(internal);

                if !sig.wait() {
                    return Err(SendError::ReceiveClosed);
                }
            }
            // Safety: it's safe to assume init as data is forgotten on another side
            Ok(unsafe { ret.assume_init() })
        }
        // if the queue is not empty send the data
    }
    /// Tries receiving from the channel within a duration
    #[inline(always)]
    pub fn recv_timeout(&self, duration: Duration) -> Result<T, SendErrorTimeout> {
        let deadline = Instant::now().checked_add(duration).unwrap();
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count == 0 {
            return Err(SendErrorTimeout::Closed);
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
                return Err(SendErrorTimeout::Timeout);
            }
            if internal.send_count == 0 {
                return Err(SendErrorTimeout::Closed);
            }
            // no active waiter so push to the queue
            let mut ret = MaybeUninit::<T>::uninit();
            let _ret_address_holder = &ret;
            {
                let sig = SyncSignal::new(ret.as_mut_ptr() as *mut T, std::thread::current());
                let _sig_address_holder = &sig;
                internal.push_recv(sig.as_signal());
                drop(internal);
                if !sig.wait_timeout(deadline) {
                    if sig.is_terminated() {
                        return Err(SendErrorTimeout::ReceiveClosed);
                    }
                    {
                        let mut internal = acquire_internal(&self.internal);
                        if internal.cancel_recv_signal(sig.as_signal()) {
                            return Err(SendErrorTimeout::Timeout);
                        }
                    }
                    // removing receive failed to wait for the signal response
                    if !sig.wait() {
                        return Err(SendErrorTimeout::ReceiveClosed);
                    }
                }
            }
            // Safety: it's safe to assume init as data is forgotten on another side
            Ok(unsafe { ret.assume_init() })
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
    /// Returns a future to receive data from the channel asynchronously
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
        ReceiveFuture {
            state: FutureState::Zero,
            sig: AsyncSignal::new(),
            internal: &self.internal,
            // Safety: data is never going to be used before receiving the signal from the sender
            data: MaybeUninit::uninit(),
        }
    }
    shared_recv_impl!();
    /// Returns sync cloned version of the receiver
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
    shared_impl!();
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count > 0 {
            internal.recv_count -= 1;
            if internal.recv_count == 0 {
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
            if internal.recv_count == 0 {
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

/// Returns bounded, sync sender and receiver of the channel for type T
/// senders and receivers can produce both async and sync versions via clone, clone_sync, and clone_async
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

/// Returns bounded, async sender and receiver of the channel for type T
/// senders and receivers can produce both async and sync versions via clone, clone_sync, and clone_async
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

/// Returns unbounded, sync sender and receiver of the channel for type T
/// senders and receivers can produce both async and sync versions via clone, clone_sync, and clone_async
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

/// Returns unbounded, async sender and receiver of the channel for type T
/// senders and receivers can produce both async and sync versions via clone, clone_sync, and clone_async
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
