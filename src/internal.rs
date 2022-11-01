use std::{collections::VecDeque, sync::Arc};

#[cfg(not(feature = "std-mutex"))]
use crate::mutex::{Mutex, MutexGuard};
#[cfg(feature = "std-mutex")]
use std::sync::{Mutex, MutexGuard};
//use spin::mutex::Mutex;

use crate::signal::Signal;

pub type Internal<T> = Arc<Mutex<ChannelInternal<T>>>;

/// Acquire mutex guard on channel internal for use in channel operations
#[inline(always)]
pub fn acquire_internal<T>(internal: &'_ Internal<T>) -> MutexGuard<'_, ChannelInternal<T>> {
    #[cfg(not(feature = "std-mutex"))]
    return internal.lock();
    #[cfg(feature = "std-mutex")]
    internal.lock().unwrap()
}

/// Tries to acquire mutex guard on channel internal for use in channel operations
#[inline(always)]
pub fn try_acquire_internal<T>(
    internal: &'_ Internal<T>,
) -> Option<MutexGuard<'_, ChannelInternal<T>>> {
    #[cfg(not(feature = "std-mutex"))]
    return internal.try_lock();
    #[cfg(feature = "std-mutex")]
    internal.try_lock().ok()
}

/// Internal of the channel that holds queues, waitlists, and general state of the channel,
///   it's shared among senders and receivers with an atomic counter and a mutex
pub struct ChannelInternal<T> {
    // KEEP THE ORDER
    /// Channel queue to save buffered objects
    pub queue: VecDeque<T>,
    /// Receive waitlist for when the channel queue is empty or zero capacity
    pub recv_wait: VecDeque<Signal<T>>,
    /// The sender waitlist for when the channel queue is full or zero capacity
    pub send_wait: VecDeque<Signal<T>>,
    /// The capacity of the channel buffer
    pub capacity: usize,
    /// Count of alive receivers
    pub recv_count: u32,
    /// Count of alive senders
    pub send_count: u32,
}

impl<T> ChannelInternal<T> {
    /// Returns a channel internal with the required capacity
    pub fn new(bounded: bool, capacity: usize) -> Internal<T> {
        let mut abstract_capacity = capacity;
        if !bounded {
            // act like there is no limit
            abstract_capacity = usize::MAX;
        }

        let ret = Self {
            queue: VecDeque::with_capacity(capacity),
            recv_wait: VecDeque::new(),
            send_wait: VecDeque::new(),
            recv_count: 1,
            send_count: 1,
            capacity: abstract_capacity,
        };

        Arc::new(Mutex::from(ret))
    }

    /// Terminates remainings signals in the queue to notify listeners about the closing of the channel
    pub fn terminate_signals(&mut self) {
        for v in &self.send_wait {
            // Safety: it's safe to terminate owned signal once
            unsafe { v.terminate() }
        }
        self.send_wait.clear();
        for v in &self.recv_wait {
            // Safety: it's safe to terminate owned signal once
            unsafe { v.terminate() }
        }
        self.recv_wait.clear();
    }

    /// Returns next signal for sender from the waitlist
    #[inline(always)]
    pub fn next_send(&mut self) -> Option<Signal<T>> {
        self.send_wait.pop_front()
    }

    /// Adds new sender signal to the waitlist
    #[inline(always)]
    pub fn push_send(&mut self, s: Signal<T>) {
        self.send_wait.push_back(s);
    }

    /// Returns the next signal for the receiver in the waitlist
    #[inline(always)]
    pub fn next_recv(&mut self) -> Option<Signal<T>> {
        self.recv_wait.pop_front()
    }

    /// Adds new receiver signal to the waitlist
    #[inline(always)]
    pub fn push_recv(&mut self, s: Signal<T>) {
        self.recv_wait.push_back(s);
    }

    /// Tries to remove the send signal from the waitlist, returns true if the operation was successful
    pub fn cancel_send_signal(&mut self, sig: Signal<T>) -> bool {
        for (i, send) in self.send_wait.iter().enumerate() {
            if sig == *send {
                self.send_wait.remove(i);
                return true;
            }
        }
        false
    }

    /// Tries to remove the received signal from the waitlist, returns true if the operation was successful
    pub fn cancel_recv_signal(&mut self, sig: Signal<T>) -> bool {
        for (i, recv) in self.recv_wait.iter().enumerate() {
            if sig == *recv {
                self.recv_wait.remove(i);
                return true;
            }
        }
        false
    }

    /// Updates the send signal in case that signal waker is changed.
    pub fn signal_exists(&mut self, sig: Signal<T>) -> bool {
        for signal in self.send_wait.iter() {
            if sig == *signal {
                return true;
            }
        }
        false
    }
}

/// Drop implementation for the channel internal, it will signal all waiters about the closing of the channel with a termination signal
impl<T> Drop for ChannelInternal<T> {
    fn drop(&mut self) {
        // in case of a drop, we should notify those who are waiting on the waitlist
        self.terminate_signals()
    }
}
