//! # Kanal: Fastest synchronous and asynchronous channel that Rust deserves.
//!
//! Kanal is a Rust library to help programmers design effective programs in CSP model via providing featureful multi-producer multi-consumer channels.
//! This library focuses on bringing both sync and async API together to unify message passing between sync and async parts of Rust code in a performant manner.
//! Performance is the main goal of Kanal.
//!

pub(crate) mod internal;
mod kanal_tests;
pub(crate) mod mutex;
mod signal;
pub(crate) mod state;

use internal::{ChannelInternal, Internal};

use std::{fmt::Debug, mem::MaybeUninit, ops::Deref, sync::atomic::AtomicPtr};

use signal::SyncSignal;

use crate::signal::AsyncSignal;

// Error states in channel
#[derive(Debug, Clone)]
pub enum Error {
    Closed,
    SendClosed,
    ReceiveClosed,
    Corrupted,
}

// Sync sender of channel for type T,, it can generate async version via clone_async
pub struct Sender<T> {
    internal: Internal<T>,
}

// Async sender of channel for type T, it can generate sync version via clone_sync
pub struct AsyncSender<T> {
    internal: Internal<T>,
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut internal = self.internal.lock();
        if internal.send_count > 0 {
            internal.send_count -= 1;
        }
    }
}

impl<T> Drop for AsyncSender<T> {
    fn drop(&mut self) {
        let mut internal = self.internal.lock();
        if internal.send_count > 0 {
            internal.send_count -= 1;
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        let mut internal = self.internal.lock();
        if internal.send_count > 0 {
            internal.send_count += 1;
        }
        Self {
            internal: self.internal.clone(),
        }
    }
}

impl<T> Clone for AsyncSender<T> {
    fn clone(&self) -> Self {
        let mut internal = self.internal.lock();
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
            self.internal.lock().capacity != usize::MAX
        }
        /// Returns length of the queue
        pub fn len(&self) -> usize {
            self.internal.lock().queue.len()
        }
        /// Returns whether channel queue is empty or not
        pub fn is_empty(&self) -> bool {
            self.internal.lock().queue.is_empty()
        }
        /// Returns capacity of channel (not the queue)
        /// for unbounded channels it will return usize::MAX
        pub fn capacity(&mut self) -> usize {
            self.internal.lock().capacity
        }
        // Closes the channel completely and terminates waiters requests
        pub fn close(&mut self) {
            let mut internal = self.internal.lock();
            internal.recv_count = 0;
            internal.send_count = 0;
            internal.terminate_signals();
            internal.send_wait.clear();
            internal.recv_wait.clear();
        }
        // Returns whether channel is closed or not
        pub fn is_closed(&mut self) -> bool {
            let internal = self.internal.lock();
            internal.send_count == 0 && internal.recv_count == 0
        }
    };
}

impl<T> Sender<T> {
    /// Sends data to the channel
    #[inline(always)]
    pub fn send(&self, mut data: T) -> Result<(), Error> {
        let mut internal = self.internal.lock();
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
            let mut sig = SyncSignal::new(&mut data as *mut T);
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
    /// Clones Sender as async version of it and returns it
    pub fn clone_async(&self) -> AsyncSender<T> {
        let mut internal = self.internal.lock();
        if internal.send_count > 0 {
            internal.send_count += 1;
        }
        AsyncSender::<T> {
            internal: self.internal.clone(),
        }
    }
    /// Returns whether receive part of channel is closed or not
    pub fn is_disconnected(&mut self) -> bool {
        self.internal.lock().recv_count == 0
    }
    shared_impl!();
}

impl<T> AsyncSender<T> {
    /// Sends data asynchronously to the channel
    #[inline(always)]
    pub async fn send(&self, mut data: T) -> Result<(), Error> {
        let mut internal = self.internal.lock();
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
            let sig = AsyncSignal::new(AtomicPtr::new(&mut data));
            internal.push_send(sig.clone().as_signal());
            drop(internal);
            let sig = sig.deref();
            let sync_wait = sig.wait_sync();
            if sync_wait >= 1 && (sync_wait == 2 || sig.await != 0) {
                return Err(Error::ReceiveClosed);
            }

            // data semantically is moved so forget about droping it if it requires droping
            if std::mem::needs_drop::<T>() {
                std::mem::forget(data);
            }
            Ok(())
        }
        // if queue is not empty send data
    }
    /// Clones async sender as sync version of it
    pub fn clone_sync(&self) -> Sender<T> {
        let mut internal = self.internal.lock();
        if internal.send_count > 0 {
            internal.send_count += 1;
        }
        Sender::<T> {
            internal: self.internal.clone(),
        }
    }
    /// Returns whether receive part of channel is closed or not
    pub fn is_disconnected(&mut self) -> bool {
        self.internal.lock().recv_count == 0
    }
    shared_impl!();
}

pub struct Receiver<T> {
    internal: Internal<T>,
}

pub struct AsyncReceiver<T> {
    internal: Internal<T>,
}

impl<T> Receiver<T> {
    /// Receives data from the channel
    #[inline(always)]
    pub fn recv(&self) -> Result<T, Error> {
        let mut internal = self.internal.lock();
        if internal.recv_count == 0 {
            return Err(Error::Closed);
        }
        if let Some(v) = internal.queue.pop_front() {
            if let Some(p) = internal.next_send() {
                // if there is a sender take it's data and push it in queue
                unsafe { internal.queue.push_back(p.recv()) }
            }
            Ok(v)
        } else if let Some(p) = internal.next_send() {
            unsafe { Ok(p.recv()) }
        } else {
            if internal.send_count == 0 {
                return Err(Error::SendClosed);
            }
            // no active waiter so push to queue
            let mut ret = MaybeUninit::<T>::uninit();
            let mut sig = SyncSignal::new(ret.as_mut_ptr() as *mut T);
            internal.push_recv(sig.as_signal());
            drop(internal);
            if !sig.wait() {
                return Err(Error::ReceiveClosed);
            }
            Ok(unsafe { ret.assume_init() })
        }
        // if queue is not empty send data
    }
    /// Returns if the send part of channel is disconnected
    pub fn is_disconnected(&mut self) -> bool {
        self.internal.lock().send_count == 0
    }
    /// Clones receiver as async version of it
    pub fn clone_async(&self) -> AsyncReceiver<T> {
        let mut internal = self.internal.lock();
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

impl<T> AsyncReceiver<T> {
    // Receives data asynchronously from the channel
    #[inline(always)]
    pub async fn recv(&self) -> Result<T, Error> {
        let mut internal = self.internal.lock();
        if internal.recv_count == 0 {
            return Err(Error::Closed);
        }
        if let Some(v) = internal.queue.pop_front() {
            if let Some(p) = internal.next_send() {
                // if there is a sender take it's data and push it in queue
                unsafe { internal.queue.push_back(p.recv()) }
            }
            Ok(v)
        } else if let Some(p) = internal.next_send() {
            unsafe { Ok(p.recv()) }
        } else {
            if internal.send_count == 0 {
                return Err(Error::SendClosed);
            }
            // no active waiter so push to queue
            let mut ret = MaybeUninit::<T>::uninit();
            let sig = AsyncSignal::new(AtomicPtr::new(ret.as_mut_ptr()));

            internal.push_recv(sig.clone().as_signal());
            drop(internal);
            let sig = sig.deref();
            let sync_wait = sig.wait_sync();
            if sync_wait >= 1 && (sync_wait == 2 || sig.await != 0) {
                return Err(Error::ReceiveClosed);
            }
            Ok(unsafe { ret.assume_init() })
        }
    }
    /// Returns sync cloned version of receiver
    pub fn clone_sync(&self) -> Receiver<T> {
        let mut internal = self.internal.lock();
        if internal.recv_count > 0 {
            internal.recv_count += 1;
        }
        Receiver::<T> {
            internal: self.internal.clone(),
        }
    }
    /// Returns whether send part of channel is closed or not
    pub fn is_disconnected(&mut self) -> bool {
        self.internal.lock().send_count == 0
    }
    shared_impl!();
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut internal = self.internal.lock();
        if internal.recv_count > 0 {
            internal.recv_count -= 1;
        }
    }
}

impl<T> Drop for AsyncReceiver<T> {
    fn drop(&mut self) {
        let mut internal = self.internal.lock();
        if internal.recv_count > 0 {
            internal.recv_count -= 1;
        }
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        let mut internal = self.internal.lock();
        if internal.recv_count > 0 {
            internal.recv_count += 1;
        }
        Self {
            internal: self.internal.clone(),
        }
    }
}

impl<T> Clone for AsyncReceiver<T> {
    fn clone(&self) -> Self {
        let mut internal = self.internal.lock();
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
pub fn unbounded_async<T>() -> (AsyncSender<T>, AsyncReceiver<T>) {
    let internal = ChannelInternal::new(false, UNBOUNDED_STARTING_SIZE);
    (
        AsyncSender {
            internal: internal.clone(),
        },
        AsyncReceiver { internal },
    )
}
