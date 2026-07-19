#[cfg(all(not(feature = "std-mutex"), not(loom)))]
use crate::mutex::{Mutex, MutexGuard};
use crate::signal::DynamicSignal;
extern crate alloc;
use alloc::collections::VecDeque;
use core::fmt::Debug;
#[cfg(all(feature = "std-mutex", not(loom)))]
use std::sync::{Mutex, MutexGuard};

use branches::unlikely;
use cacheguard::CacheGuard;
// Loom's model-checked mutex replaces both mutex flavors under `--cfg
// loom`; the kanal RawMutexLock itself is model-checked separately in
// src/mutex.rs.
#[cfg(loom)]
use loom::sync::{Mutex, MutexGuard};

pub(crate) struct Internal<T> {
    // The cache guard to keep internal pointer in cache, it works with both
    // std mutex and mutex.rs
    _guard: CacheGuard<()>,
    /// The internal channel object
    internal: *mut (Mutex<ChannelInternal<T>>, usize),
}

impl<T> Debug for Internal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Internal").finish()
    }
}

// Mutex is sync and send, so we can send it across threads
unsafe impl<T: Send> Send for Internal<T> {}
// T: Send is required: a shared &Sender/&Receiver moves T values across
// threads, so an unbounded Sync impl would let !Send types (e.g. Rc) cross
// thread boundaries through safe code.
unsafe impl<T: Send> Sync for Internal<T> {}

impl<T> Internal<T> {
    #[inline(always)]
    pub(crate) unsafe fn drop(&self) {
        let _ = Box::from_raw(self.internal);
    }
    /// Returns a channel internal with the required capacity
    #[inline(always)]
    pub(crate) fn new(bounded: bool, capacity: usize) -> Internal<T> {
        let mut abstract_capacity = capacity;
        if !bounded {
            // act like there is no limit
            abstract_capacity = usize::MAX;
        }
        let wait_list_size = if capacity == 0 { 8 } else { 4 };
        let ret = ChannelInternal {
            queue: VecDeque::with_capacity(capacity),
            recv_blocking: false,
            wait_list: VecDeque::with_capacity(wait_list_size),
            recv_count: 1,
            send_count: 1,
            // always has the default one sender and one receiver
            ref_count: 2,
        };

        Internal {
            _guard: CacheGuard::new(()),
            internal: Box::into_raw(Box::new((
                Mutex::new(ret),
                abstract_capacity,
            ))),
        }
    }

    #[inline(always)]
    pub(crate) fn capacity(&self) -> usize {
        // SAFETY: capacity is stored alongside the internal mutex as a read
        // only data
        unsafe { (*self.internal).1 }
    }

    #[inline(always)]
    pub(crate) fn clone_recv(&self) -> Internal<T> {
        acquire_internal(self).inc_ref_count(false);
        Internal {
            _guard: CacheGuard::new(()),
            internal: self.internal,
        }
    }

    #[inline(always)]
    pub(crate) fn drop_recv(&mut self) {
        let mut internal = acquire_internal(self);
        let (is_last, cleanup) = internal.dec_ref_count(false);
        drop(internal);
        if let Some(cleanup) = cleanup {
            cleanup.run();
        }
        if unlikely(is_last) {
            unsafe { self.drop() }
        }
    }

    #[inline(always)]
    pub(crate) fn clone_send(&self) -> Internal<T> {
        acquire_internal(self).inc_ref_count(true);
        Internal {
            _guard: CacheGuard::new(()),
            internal: self.internal,
        }
    }

    #[inline(always)]
    pub(crate) fn drop_send(&mut self) {
        let mut internal = acquire_internal(self);
        let (is_last, cleanup) = internal.dec_ref_count(true);
        drop(internal);
        if let Some(cleanup) = cleanup {
            cleanup.run();
        }
        if unlikely(is_last) {
            unsafe { self.drop() }
        }
    }

    #[inline(always)]
    pub(crate) fn clone_unchecked(&self) -> Internal<T> {
        Internal {
            _guard: CacheGuard::new(()),
            internal: self.internal,
        }
    }
}

impl<T> core::ops::Deref for Internal<T> {
    type Target = Mutex<ChannelInternal<T>>;
    fn deref(&self) -> &Self::Target {
        unsafe { &(*self.internal).0 }
    }
}

/// Acquire mutex guard on channel internal for use in channel operations
#[inline(always)]
pub(crate) fn acquire_internal<T>(
    internal: &'_ Internal<T>,
) -> MutexGuard<'_, ChannelInternal<T>> {
    #[cfg(all(not(feature = "std-mutex"), not(loom)))]
    return internal.lock();
    #[cfg(any(feature = "std-mutex", loom))]
    internal.lock().unwrap_or_else(|err| err.into_inner())
}

/// Tries to acquire mutex guard on channel internal for use in channel
/// operations
#[inline(always)]
pub(crate) fn try_acquire_internal<T>(
    internal: &'_ Internal<T>,
) -> Option<MutexGuard<'_, ChannelInternal<T>>> {
    #[cfg(all(not(feature = "std-mutex"), not(loom)))]
    return internal.try_lock();
    #[cfg(any(feature = "std-mutex", loom))]
    internal.try_lock().ok()
}

/// Internal of the channel that holds queues, waitlists, and general state of
/// the channel, it's shared among senders and receivers with an atomic
/// counter and a mutex
pub(crate) struct ChannelInternal<T> {
    // KEEP THE ORDER
    /// Channel queue to save buffered objects
    pub(crate) queue: VecDeque<T>,
    /// It's true if the signals in the waiting list are recv signals
    pub(crate) recv_blocking: bool,
    /// Receive and Send waitlist for when the channel queue is empty or zero
    /// capacity for recv or full for send.
    pub(crate) wait_list: VecDeque<DynamicSignal<T>>,
    /// Count of alive receivers
    pub(crate) recv_count: u32,
    /// Count of alive senders
    pub(crate) send_count: u32,
    /// Reference counter for the alive instances of the channel
    pub(crate) ref_count: usize,
}

/// Termination work detached from the channel while its lock was held; the
/// caller must `run` it after releasing the lock so that unpark/wake side
/// effects and drops of queued user data never execute inside the critical
/// section.
pub(crate) struct Cleanup<T> {
    wait_list: VecDeque<DynamicSignal<T>>,
    queue: VecDeque<T>,
}

impl<T> Cleanup<T> {
    /// Terminates the detached signals and drops the detached queue. Must be
    /// called without holding the channel lock.
    pub(crate) fn run(self) {
        for t in self.wait_list.iter() {
            // SAFETY: the signals were detached from the waitlist under the
            // lock, their owners can no longer cancel them, and this side
            // terminates each of them exactly once.
            unsafe { t.terminate() }
        }
        drop(self.queue);
    }
}

impl<T> ChannelInternal<T> {
    /// Detaches all waiting signals and optionally the queued data for
    /// termination, so the caller can terminate/drop them after releasing
    /// the channel lock.
    #[cold]
    pub(crate) fn detach_cleanup(&mut self, take_queue: bool) -> Cleanup<T> {
        Cleanup {
            wait_list: core::mem::take(&mut self.wait_list),
            queue: if take_queue {
                core::mem::take(&mut self.queue)
            } else {
                VecDeque::new()
            },
        }
    }

    /// Returns next signal for sender from the waitlist
    #[inline(always)]
    pub(crate) fn next_send(&mut self) -> Option<DynamicSignal<T>> {
        if self.recv_blocking {
            return None;
        }
        match self.wait_list.pop_front() {
            Some(sig) => Some(sig),
            None => {
                self.recv_blocking = true;
                None
            }
        }
    }

    /// Removes up to `max` waiting receive signals from the waitlist and
    /// returns them. The caller owns the returned signals and must complete
    /// every one of them; completing them after releasing the channel lock
    /// is safe because detached signals can no longer be canceled. Like
    /// [`Self::next_recv`], an exhausted waitlist lazily flips over to the
    /// send side.
    #[inline(always)]
    pub(crate) fn take_recvs(
        &mut self,
        max: usize,
    ) -> VecDeque<DynamicSignal<T>> {
        if !self.recv_blocking {
            return VecDeque::new();
        }
        if self.wait_list.is_empty() {
            self.recv_blocking = false;
            return VecDeque::new();
        }
        if max >= self.wait_list.len() {
            core::mem::take(&mut self.wait_list)
        } else {
            let tail = self.wait_list.split_off(max);
            core::mem::replace(&mut self.wait_list, tail)
        }
    }

    /// Removes all waiting send signals from the waitlist and returns them.
    /// The caller owns the returned signals and must complete every one of
    /// them; completing them after releasing the channel lock is safe
    /// because detached signals can no longer be canceled. Like
    /// [`Self::next_send`], an exhausted waitlist lazily flips over to the
    /// receive side.
    #[inline(always)]
    pub(crate) fn take_sends(&mut self) -> VecDeque<DynamicSignal<T>> {
        if self.recv_blocking {
            return VecDeque::new();
        }
        if self.wait_list.is_empty() {
            self.recv_blocking = true;
            return VecDeque::new();
        }
        core::mem::take(&mut self.wait_list)
    }

    /// Adds new sender/receiver signal to the waitlist
    #[inline(always)]
    pub(crate) fn push_signal(&mut self, s: DynamicSignal<T>) {
        self.wait_list.push_back(s);
    }

    /// Returns the next signal for the receiver in the waitlist
    #[inline(always)]
    pub(crate) fn next_recv(&mut self) -> Option<DynamicSignal<T>> {
        if !self.recv_blocking {
            return None;
        }
        match self.wait_list.pop_front() {
            Some(sig) => Some(sig),
            None => {
                self.recv_blocking = false;
                None
            }
        }
    }

    /// Tries to remove the send signal from the waitlist, returns true if the
    /// operation was successful
    pub(crate) fn cancel_send_signal(&mut self, sig: *const ()) -> bool {
        if !self.recv_blocking {
            for (i, send) in self.wait_list.iter().enumerate() {
                if send.eq_ptr(sig) {
                    // SAFETY: it's safe to cancel owned signal once, we are
                    // sure that index is valid
                    unsafe {
                        self.wait_list.remove(i).unwrap_unchecked().cancel();
                    }
                    return true;
                }
            }
        }
        false
    }

    /// Tries to remove the received signal from the waitlist, returns true if
    /// the operation was successful
    pub(crate) fn cancel_recv_signal(&mut self, sig: *const ()) -> bool {
        if self.recv_blocking {
            for (i, recv) in self.wait_list.iter().enumerate() {
                if recv.eq_ptr(sig) {
                    // SAFETY: it's safe to cancel owned signal once, we are
                    // sure that index is valid
                    unsafe {
                        self.wait_list.remove(i).unwrap_unchecked().cancel();
                    }
                    return true;
                }
            }
        }
        false
    }

    /// checks if send signal exists in wait list
    #[cfg(feature = "async")]
    pub(crate) fn send_signal_exists(&self, sig: *const ()) -> bool {
        if !self.recv_blocking {
            for signal in self.wait_list.iter() {
                if signal.eq_ptr(sig) {
                    return true;
                }
            }
        }
        false
    }

    /// checks if receive signal exists in wait list
    #[cfg(feature = "async")]
    pub(crate) fn recv_signal_exists(&self, sig: *const ()) -> bool {
        if self.recv_blocking {
            for signal in self.wait_list.iter() {
                if signal.eq_ptr(sig) {
                    return true;
                }
            }
        }
        false
    }

    /// Increases ref count for sender or receiver
    #[inline(always)]
    pub(crate) fn inc_ref_count(&mut self, is_sender: bool) {
        if is_sender {
            if self.send_count > 0 {
                self.send_count = self
                    .send_count
                    .checked_add(1)
                    .expect("channel sender count overflow");
            }
        } else if self.recv_count > 0 {
            self.recv_count = self
                .recv_count
                .checked_add(1)
                .expect("channel receiver count overflow");
        }
        self.ref_count += 1;
    }

    /// Decreases ref count for sender or receiver, returning whether this
    /// was the last instance of the channel alongside any cleanup that must
    /// run after the channel lock is released.
    #[inline(always)]
    pub(crate) fn dec_ref_count(
        &mut self,
        is_sender: bool,
    ) -> (bool, Option<Cleanup<T>>) {
        let mut cleanup = None;
        if is_sender {
            if self.send_count > 0 {
                self.send_count -= 1;
                if self.send_count == 0 && self.recv_count != 0 {
                    cleanup = Some(self.detach_cleanup(false));
                }
            }
        } else if self.recv_count > 0 {
            self.recv_count -= 1;
            if self.recv_count == 0 {
                cleanup = Some(self.detach_cleanup(true));
            }
        }
        self.ref_count -= 1;
        (self.ref_count == 0, cleanup)
    }
}
