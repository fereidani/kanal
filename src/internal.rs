#[cfg(not(feature = "std-mutex"))]
use crate::mutex::{Mutex, MutexGuard};
use crate::signal::{Signal, SignalTerminator};
#[cfg(feature = "std-mutex")]
use std::sync::{Mutex, MutexGuard};
use std::{collections::VecDeque, sync::Arc};

pub(crate) type Internal<T> = Arc<Mutex<ChannelInternal<T>>>;

/// Acquire mutex guard on channel internal for use in channel operations
#[inline(always)]
pub(crate) fn acquire_internal<T>(internal: &'_ Internal<T>) -> MutexGuard<'_, ChannelInternal<T>> {
    #[cfg(not(feature = "std-mutex"))]
    return internal.lock();
    #[cfg(feature = "std-mutex")]
    internal.lock().unwrap()
}

/// Tries to acquire mutex guard on channel internal for use in channel operations
#[inline(always)]
pub(crate) fn try_acquire_internal<T>(
    internal: &'_ Internal<T>,
) -> Option<MutexGuard<'_, ChannelInternal<T>>> {
    #[cfg(not(feature = "std-mutex"))]
    return internal.try_lock();
    #[cfg(feature = "std-mutex")]
    internal.try_lock().ok()
}

/// Internal of the channel that holds queues, waitlists, and general state of the channel,
///   it's shared among senders and receivers with an atomic counter and a mutex
pub(crate) struct ChannelInternal<T> {
    // KEEP THE ORDER
    /// Channel queue to save buffered objects
    pub(crate) queue: VecDeque<T>,
    /// Receive waitlist for when the channel queue is empty or zero capacity
    pub(crate) recv_wait: VecDeque<SignalTerminator<T>>,
    /// The sender waitlist for when the channel queue is full or zero capacity
    pub(crate) send_wait: VecDeque<SignalTerminator<T>>,
    /// The capacity of the channel buffer
    pub(crate) capacity: usize,
    /// Count of alive receivers
    pub(crate) recv_count: u32,
    /// Count of alive senders
    pub(crate) send_count: u32,
}

// Safety: safety of channel internal movement is
unsafe impl<T> Send for ChannelInternal<T> {}

impl<T> ChannelInternal<T> {
    /// Returns a channel internal with the required capacity
    pub(crate) fn new(bounded: bool, capacity: usize) -> Internal<T> {
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
    pub(crate) fn terminate_signals(&mut self) {
        for t in self.send_wait.iter() {
            // Safety: it's safe to terminate owned signal once
            unsafe { t.terminate() }
        }
        self.send_wait.clear();
        for t in self.recv_wait.iter() {
            // Safety: it's safe to terminate owned signal once
            unsafe { t.terminate() }
        }
        self.recv_wait.clear();
    }

    /// Returns next signal for sender from the waitlist
    #[inline(always)]
    pub(crate) fn next_send(&mut self) -> Option<SignalTerminator<T>> {
        self.send_wait.pop_front()
    }

    /// Adds new sender signal to the waitlist
    #[inline(always)]
    pub(crate) fn push_send(&mut self, s: SignalTerminator<T>) {
        self.send_wait.push_back(s);
    }

    /// Returns the next signal for the receiver in the waitlist
    #[inline(always)]
    pub(crate) fn next_recv(&mut self) -> Option<SignalTerminator<T>> {
        self.recv_wait.pop_front()
    }

    /// Adds new receiver signal to the waitlist
    #[inline(always)]
    pub(crate) fn push_recv(&mut self, s: SignalTerminator<T>) {
        self.recv_wait.push_back(s);
    }

    /// Tries to remove the send signal from the waitlist, returns true if the operation was successful
    pub(crate) fn cancel_send_signal(&mut self, sig: &Signal<T>) -> bool {
        for (i, send) in self.send_wait.iter().enumerate() {
            if send.eq(sig) {
                self.send_wait.remove(i);
                return true;
            }
        }
        false
    }

    /// Tries to remove the received signal from the waitlist, returns true if the operation was successful
    pub(crate) fn cancel_recv_signal(&mut self, sig: &Signal<T>) -> bool {
        for (i, recv) in self.recv_wait.iter().enumerate() {
            if recv.eq(sig) {
                self.recv_wait.remove(i);
                return true;
            }
        }
        false
    }

    /// checks if send signal exists in wait list
    #[cfg(feature = "async")]
    pub(crate) fn send_signal_exists(&self, sig: &Signal<T>) -> bool {
        for signal in self.send_wait.iter() {
            if signal.eq(sig) {
                return true;
            }
        }
        false
    }

    /// checks if receive signal exists in wait list
    #[cfg(feature = "async")]
    pub(crate) fn recv_signal_exists(&self, sig: &Signal<T>) -> bool {
        for signal in self.recv_wait.iter() {
            if signal.eq(sig) {
                return true;
            }
        }
        false
    }
}
