//! # Kanal: The fast synchronous and asynchronous channel that Rust deserves.
//!
//! Kanal is a Rust library to help programmers design effective programs in CSP model via providing featureful multi-producer multi-consumer channels.
//! This library focuses on bringing both sync and async API together to unify message passing between sync and async parts of Rust code in a performant manner.
//! Performance is the main goal of Kanal.
//!
//!
#[cfg(feature = "async")]
mod future;
#[cfg(feature = "async")]
pub use future::*;

pub(crate) mod internal;
mod kanal_tests;
pub(crate) mod mutex;
mod signal;
pub(crate) mod state;

use internal::{acquire_internal, ChannelInternal, Internal};

#[cfg(feature = "async")]
use std::mem::ManuallyDrop;
use std::{
    mem::MaybeUninit,
    time::{Duration, SystemTime},
};

use std::fmt;
use std::fmt::Debug;

#[cfg(feature = "async")]
use signal::AsyncSignal;
use signal::SyncSignal;

/*
use crate::mutex::MutexGuard;
#[inline(always)]
fn acquire_internal<'a, T>(internal: &'a Internal<T>) -> MutexGuard<'a, ChannelInternal<T>> {
    internal.lock()
}*/

// Error states in channel
#[derive(Debug)]
pub enum Error {
    Closed,
    SendClosed,
    ReceiveClosed,
}
impl std::error::Error for Error {}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(
            match *self {
                Error::Closed => "channel is closed",
                Error::SendClosed => "channel send side is closed",
                Error::ReceiveClosed => "channel receive side is closed",
            },
            f,
        )
    }
}

// Error states in channel
#[derive(Debug)]
pub enum ErrorTimeout {
    Closed,
    SendClosed,
    ReceiveClosed,
    Timeout,
}
impl std::error::Error for ErrorTimeout {}
impl fmt::Display for ErrorTimeout {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(
            match *self {
                ErrorTimeout::Closed => "channel is closed",
                ErrorTimeout::SendClosed => "channel send side is closed",
                ErrorTimeout::ReceiveClosed => "channel receive side is closed",
                ErrorTimeout::Timeout => "channel operation timeout",
            },
            f,
        )
    }
}
// Sync sender of channel for type T, it can generate async version via clone_async
pub struct Sender<T> {
    internal: Internal<T>,
}

// Async sender of channel for type T, it can generate sync version via clone_sync
#[cfg(feature = "async")]
pub struct AsyncSender<T> {
    internal: Internal<T>,
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count > 0 {
            internal.send_count -= 1;
        }
    }
}

#[cfg(feature = "async")]
impl<T> Drop for AsyncSender<T> {
    fn drop(&mut self) {
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count > 0 {
            internal.send_count -= 1;
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

macro_rules! shared_impl {
    () => {
        /// Returns whether channel is bounded or not
        pub fn is_bounded(&mut self) -> bool {
            acquire_internal(&self.internal).capacity != usize::MAX
        }
        /// Returns length of the queue
        pub fn len(&self) -> usize {
            acquire_internal(&self.internal).queue.len()
        }
        /// Returns whether channel queue is empty or not
        pub fn is_empty(&self) -> bool {
            acquire_internal(&self.internal).queue.is_empty()
        }
        /// Returns capacity of channel (not the queue)
        /// for unbounded channels it will return usize::MAX
        pub fn capacity(&mut self) -> usize {
            acquire_internal(&self.internal).capacity
        }
        // Closes the channel completely and terminates waiters requests
        pub fn close(&mut self) {
            let mut internal = acquire_internal(&self.internal);
            internal.recv_count = 0;
            internal.send_count = 0;
            internal.terminate_signals();
            internal.send_wait.clear();
            internal.recv_wait.clear();
        }
        // Returns whether channel is closed or not
        pub fn is_closed(&mut self) -> bool {
            let internal = acquire_internal(&self.internal);
            internal.send_count == 0 && internal.recv_count == 0
        }
    };
}

impl<T> Sender<T> {
    /// Sends data to the channel
    #[inline(always)]
    pub fn send(&self, mut data: T) -> Result<(), Error> {
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count == 0 {
            return Err(Error::Closed);
        }
        if let Some(first) = internal.next_recv() {
            drop(internal);
            unsafe { first.send(data) }
            Ok(())
        } else if internal.queue.len() < internal.capacity {
            internal.queue.push_back(data);
            Ok(())
        } else {
            if internal.recv_count == 0 {
                return Err(Error::ReceiveClosed);
            }
            // send directly to wait list

            let sig = SyncSignal::new(&mut data as *mut T, std::thread::current());
            internal.push_send(sig.as_signal());
            drop(internal);
            if !sig.wait() {
                return Err(Error::SendClosed);
            }
            // data semantically is moved so forget about droping it if it requires droping
            if std::mem::needs_drop::<T>() {
                std::mem::forget(data);
            }
            Ok(())
        }
        // if queue is not empty send data
    }
    /// Sends data to the channel
    #[inline(always)]
    pub fn send_timeout(&self, mut data: T, duration: Duration) -> Result<(), ErrorTimeout> {
        let deadline = SystemTime::now().checked_add(duration).unwrap();
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count == 0 {
            return Err(ErrorTimeout::Closed);
        }
        if let Some(first) = internal.next_recv() {
            drop(internal);
            unsafe { first.send(data) }
            Ok(())
        } else if internal.queue.len() < internal.capacity {
            internal.queue.push_back(data);
            Ok(())
        } else {
            if internal.recv_count == 0 {
                return Err(ErrorTimeout::ReceiveClosed);
            }
            // send directly to wait list

            let sig = SyncSignal::new(&mut data as *mut T, std::thread::current());
            let cancelable_sig = sig.as_signal();
            internal.push_send(cancelable_sig);
            drop(internal);
            if !sig.wait_timeout(deadline) {
                if sig.is_terminated() {
                    return Err(ErrorTimeout::SendClosed);
                }
                {
                    let mut internal = acquire_internal(&self.internal);
                    if internal.cancel_send_signal(sig.as_signal()) {
                        return Err(ErrorTimeout::Timeout);
                    }
                }
                // removing receive failed wait for signal response
                if !sig.wait() {
                    return Err(ErrorTimeout::SendClosed);
                }
            }
            // data semantically is moved so forget about droping it if it requires droping
            if std::mem::needs_drop::<T>() {
                std::mem::forget(data);
            }
            Ok(())
        }
        // if queue is not empty send data
    }
    #[inline(always)]
    pub fn send_option_timeout(
        &self,
        data: &mut Option<T>,
        duration: Duration,
    ) -> Result<(), ErrorTimeout> {
        let deadline = SystemTime::now().checked_add(duration).unwrap();
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count == 0 {
            return Err(ErrorTimeout::Closed);
        }
        if let Some(first) = internal.next_recv() {
            drop(internal);
            unsafe { first.send(data.take().unwrap()) }
            Ok(())
        } else if internal.queue.len() < internal.capacity {
            internal.queue.push_back(data.take().unwrap());
            Ok(())
        } else {
            if internal.recv_count == 0 {
                return Err(ErrorTimeout::ReceiveClosed);
            }
            // send directly to wait list
            let mut d = data.take().unwrap();
            let sig = SyncSignal::new(&mut d as *mut T, std::thread::current());
            internal.push_send(sig.as_signal());
            drop(internal);
            if !sig.wait_timeout(deadline) {
                if sig.is_terminated() {
                    *data = Some(d);
                    return Err(ErrorTimeout::SendClosed);
                }
                {
                    let mut internal = acquire_internal(&self.internal);
                    if internal.cancel_send_signal(sig.as_signal()) {
                        *data = Some(d);
                        return Err(ErrorTimeout::Timeout);
                    }
                }
                // removing receive failed wait for signal response
                if !sig.wait() {
                    *data = Some(d);
                    return Err(ErrorTimeout::SendClosed);
                }
            }
            Ok(())
        }
        // if queue is not empty send data
    }

    /// Tries sending to the channel without waiting in the wait list
    /// It returns Ok(true) in case of successful operation and Ok(false) for failed one, or error in case that channel is closed
    /// Important note: this function is not lock free as it acquire mutex of channel internal for a short time.
    #[inline(always)]
    pub fn try_send(&self, data: T) -> Result<bool, Error> {
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count == 0 {
            return Err(Error::Closed);
        }
        if let Some(first) = internal.next_recv() {
            drop(internal);
            unsafe { first.send(data) }
            return Ok(true);
        } else if internal.queue.len() < internal.capacity {
            internal.queue.push_back(data);
            return Ok(true);
        }
        Ok(false)
    }

    /// Clones Sender as async version of it and returns it
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
    /// Returns whether receive part of channel is closed or not
    pub fn is_disconnected(&mut self) -> bool {
        acquire_internal(&self.internal).recv_count == 0
    }
    shared_impl!();
}

#[cfg(feature = "async")]
impl<T> AsyncSender<T> {
    /// Sends data asynchronously to the channel
    #[inline(always)]
    pub fn send(&'_ self, data: T) -> SendFuture<'_, T> {
        SendFuture {
            state: FutureState::Zero,
            internal: &self.internal,
            sig: AsyncSignal::new(),
            data: ManuallyDrop::new(data),
        }
    }
    /// Tries sending to the channel without waiting in the wait list
    /// It returns Ok(true) in case of successful operation and Ok(false) for failed one, or error in case that channel is closed
    /// Important note: this function is not lock free as it acquire mutex of channel internal for a short time.
    #[inline(always)]
    pub fn try_send(&self, data: T) -> Result<bool, Error> {
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count == 0 {
            return Err(Error::Closed);
        }
        if let Some(first) = internal.next_recv() {
            drop(internal);
            unsafe { first.send(data) }
            return Ok(true);
        } else if internal.queue.len() < internal.capacity {
            internal.queue.push_back(data);
            return Ok(true);
        }
        Ok(false)
    }
    #[inline(always)]
    pub fn try_send_option(&self, data: &mut Option<T>) -> Result<bool, Error> {
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count == 0 {
            return Err(Error::Closed);
        }
        if let Some(first) = internal.next_recv() {
            drop(internal);
            unsafe { first.send(data.take().unwrap()) }
            return Ok(true);
        } else if internal.queue.len() < internal.capacity {
            internal.queue.push_back(data.take().unwrap());
            return Ok(true);
        }
        Ok(false)
    }
    /// Clones async sender as sync version of it
    pub fn clone_sync(&self) -> Sender<T> {
        let mut internal = acquire_internal(&self.internal);
        if internal.send_count > 0 {
            internal.send_count += 1;
        }
        Sender::<T> {
            internal: self.internal.clone(),
        }
    }

    /// Returns whether receive part of channel is closed or not
    pub fn is_disconnected(&mut self) -> bool {
        acquire_internal(&self.internal).recv_count == 0
    }
    shared_impl!();
}

pub struct Receiver<T> {
    internal: Internal<T>,
}

#[cfg(feature = "async")]
pub struct AsyncReceiver<T> {
    internal: Internal<T>,
}

impl<T> Receiver<T> {
    /// Receives data from the channel
    #[inline(always)]
    pub fn recv(&self) -> Result<T, Error> {
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count == 0 {
            return Err(Error::Closed);
        }
        if let Some(v) = internal.queue.pop_front() {
            if let Some(p) = internal.next_send() {
                // if there is a sender take its data and push it in queue
                unsafe { internal.queue.push_back(p.recv()) }
            }
            Ok(v)
        } else if let Some(p) = internal.next_send() {
            drop(internal);
            unsafe { Ok(p.recv()) }
        } else {
            if internal.send_count == 0 {
                return Err(Error::SendClosed);
            }
            // no active waiter so push to queue
            let mut ret = MaybeUninit::<T>::uninit();
            let sig = SyncSignal::new(ret.as_mut_ptr(), std::thread::current());
            internal.push_recv(sig.as_signal());
            drop(internal);

            if !sig.wait() {
                return Err(Error::ReceiveClosed);
            }
            Ok(unsafe { ret.assume_init() })
        }
        // if queue is not empty send data
    }
    #[inline(always)]
    pub fn recv_timeout(&self, duration: Duration) -> Result<T, ErrorTimeout> {
        let deadline = SystemTime::now().checked_add(duration).unwrap();
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count == 0 {
            return Err(ErrorTimeout::Closed);
        }
        if let Some(v) = internal.queue.pop_front() {
            if let Some(p) = internal.next_send() {
                // if there is a sender take its data and push it in queue
                unsafe { internal.queue.push_back(p.recv()) }
            }
            Ok(v)
        } else if let Some(p) = internal.next_send() {
            drop(internal);
            unsafe { Ok(p.recv()) }
        } else {
            if SystemTime::now() > deadline {
                return Err(ErrorTimeout::Timeout);
            }
            if internal.send_count == 0 {
                return Err(ErrorTimeout::SendClosed);
            }
            // no active waiter so push to queue
            let mut ret = MaybeUninit::<T>::uninit();
            let sig = SyncSignal::new(ret.as_mut_ptr() as *mut T, std::thread::current());
            internal.push_recv(sig.as_signal());
            drop(internal);
            if !sig.wait_timeout(deadline) {
                if sig.is_terminated() {
                    return Err(ErrorTimeout::ReceiveClosed);
                }
                {
                    let mut internal = acquire_internal(&self.internal);
                    if internal.cancel_recv_signal(sig.as_signal()) {
                        return Err(ErrorTimeout::Timeout);
                    }
                }
                // removing receive failed wait for signal response
                if !sig.wait() {
                    return Err(ErrorTimeout::ReceiveClosed);
                }
            }
            Ok(unsafe { ret.assume_init() })
        }
        // if queue is not empty send data
    }
    /// Tries receiving from the channel without waiting in the wait list
    /// It returns Ok(Some(T)) in case of successful operation and Ok(None) for failed one, or error in case that channel is closed
    /// Important note: this function is not lock free as it acquire mutex of channel internal for a short time.
    #[inline(always)]
    pub fn try_recv(&self) -> Result<Option<T>, Error> {
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count == 0 {
            return Err(Error::Closed);
        }
        if let Some(v) = internal.queue.pop_front() {
            if let Some(p) = internal.next_send() {
                // if there is a sender take its data and push it in queue
                unsafe { internal.queue.push_back(p.recv()) }
            }
            return Ok(Some(v));
        } else if let Some(p) = internal.next_send() {
            return unsafe { Ok(Some(p.recv())) };
        }
        Ok(None)
        // if queue is not empty send data
    }
    /// Returns if the send part of channel is disconnected
    pub fn is_disconnected(&mut self) -> bool {
        acquire_internal(&self.internal).send_count == 0
    }
    #[cfg(feature = "async")]
    /// Clones receiver as async version of it
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
    // Receives data asynchronously from the channel
    #[inline(always)]
    pub fn recv(&'_ self) -> ReceiveFuture<'_, T> {
        ReceiveFuture {
            state: FutureState::Zero,
            sig: AsyncSignal::new(),
            internal: &self.internal,
            data: ManuallyDrop::new(unsafe { std::mem::zeroed() }),
        }
    }
    /// Tries receiving from the channel without waiting in the wait list
    /// It returns Ok(Some(T)) in case of successful operation and Ok(None) for failed one, or error in case that channel is closed
    /// Important note: this function is not lock free as it acquire mutex of channel internal for a short time.
    #[inline(always)]
    pub fn try_recv(&self) -> Result<Option<T>, Error> {
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count == 0 {
            return Err(Error::Closed);
        }
        if let Some(v) = internal.queue.pop_front() {
            if let Some(p) = internal.next_send() {
                // if there is a sender take its data and push it in queue
                unsafe { internal.queue.push_back(p.recv()) }
            }
            return Ok(Some(v));
        } else if let Some(p) = internal.next_send() {
            return unsafe { Ok(Some(p.recv())) };
        }
        Ok(None)
        // if queue is not empty send data
    }
    /// Returns sync cloned version of receiver
    pub fn clone_sync(&self) -> Receiver<T> {
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count > 0 {
            internal.recv_count += 1;
        }
        Receiver::<T> {
            internal: self.internal.clone(),
        }
    }
    /// Returns whether send part of channel is closed or not
    pub fn is_disconnected(&mut self) -> bool {
        acquire_internal(&self.internal).send_count == 0
    }
    shared_impl!();
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count > 0 {
            internal.recv_count -= 1;
        }
    }
}

#[cfg(feature = "async")]
impl<T> Drop for AsyncReceiver<T> {
    fn drop(&mut self) {
        let mut internal = acquire_internal(&self.internal);
        if internal.recv_count > 0 {
            internal.recv_count -= 1;
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
/// senders and receivers can produce both async and sync version via clone, clone_sync and clone_async
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
/// senders and receivers can produce both async and sync version via clone, clone_sync and clone_async
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
/// senders and receivers can produce both async and sync version via clone, clone_sync and clone_async
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
/// senders and receivers can produce both async and sync version via clone, clone_sync and clone_async
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
