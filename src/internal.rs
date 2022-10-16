use std::{collections::VecDeque, sync::Arc};

use crate::mutex::{Mutex, MutexGuard};
//use std::sync::{Mutex, MutexGuard};
//use spin::mutex::Mutex;

use crate::signal::Signal;

pub type Internal<T> = Arc<Mutex<ChannelInternal<T>>>;

#[inline(always)]
pub fn acquire_internal<T>(internal: &'_ Internal<T>) -> MutexGuard<'_, ChannelInternal<T>> {
    internal.lock()
}

/// Internal of channel that holds queues, wait lists and general state of channel,
///   it's shared among senders and receivers with an atomic counter and a mutex
pub struct ChannelInternal<T> {
    // KEEP THE ORDER
    pub queue: VecDeque<T>,
    // receiver will wait until queue becomes full
    pub recv_wait: VecDeque<Signal<T>>,
    // sender will wait until queue becomes empty
    pub send_wait: VecDeque<Signal<T>>,
    pub capacity: usize,
    pub recv_count: u32,
    pub send_count: u32,
}

impl<T> ChannelInternal<T> {
    /// Returns a channel internal with required capacity
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

    /// Terminates remainings signals in queue to notify listeners about closing of channel
    pub fn terminate_signals(&self) {
        for v in &self.send_wait {
            unsafe { v.terminate() }
        }
        for v in &self.recv_wait {
            unsafe { v.terminate() }
        }
    }

    /// Returns next signal for sender from wait list
    #[inline(always)]
    pub fn next_send(&mut self) -> Option<Signal<T>> {
        self.send_wait.pop_front()
    }

    /// Adds new sender signal to wait list
    #[inline(always)]
    pub fn push_send(&mut self, s: Signal<T>) {
        self.send_wait.push_back(s);
    }

    /// Returns next signal for receiver in wait list
    #[inline(always)]
    pub fn next_recv(&mut self) -> Option<Signal<T>> {
        self.recv_wait.pop_front()
    }

    /// Adds new receiver signal to wait list
    #[inline(always)]
    pub fn push_recv(&mut self, s: Signal<T>) {
        self.recv_wait.push_back(s);
    }

    #[inline(always)]
    pub fn cancel_send_signal(&mut self, sig: Signal<T>) -> bool {
        for (i, send) in self.send_wait.iter().enumerate() {
            if sig == *send {
                self.send_wait.remove(i);
                return true;
            }
        }
        false
    }

    #[inline(always)]
    pub fn cancel_recv_signal(&mut self, sig: Signal<T>) -> bool {
        for (i, recv) in self.recv_wait.iter().enumerate() {
            if sig == *recv {
                self.recv_wait.remove(i);
                return true;
            }
        }
        false
    }
}

impl<T> Drop for ChannelInternal<T> {
    fn drop(&mut self) {
        // in case of drop we should notify those who are waiting in wait lists.
        self.terminate_signals()
    }
}
