//! Loom model tests for the channel protocol.
//!
//! These tests only exist when the crate is compiled with
//! `RUSTFLAGS="--cfg loom"`; loom then replaces the concurrency primitives
//! (see `src/primitives.rs`) and explores the possible interleavings of
//! every model below under the C11 memory model, including data-race
//! detection on the signal/KanalPtr cells and lost-wakeup/deadlock
//! detection around parking.
//!
//! Run with:
//! `RUSTFLAGS="--cfg loom" cargo test --release --test loom --all-features`
//!
//! Models must stay tiny (2-3 threads, a couple of messages) as the state
//! space grows exponentially. Timeout-based APIs must not be used: loom
//! does not model time and the loom build waits without a timeout.
#![cfg(loom)]

use loom::thread;

/// A payload bigger than a pointer, forcing the KanalPtr protocol to move
/// the data through the pointed-to stack slot instead of serializing it
/// inside the pointer itself.
#[derive(Debug, PartialEq, Eq)]
struct Big([usize; 4]);

impl Big {
    fn new(v: usize) -> Self {
        Big([v; 4])
    }
}

/// Zero-capacity rendezvous handoff of a pointer-sized payload: the value
/// is serialized inside the signal pointer itself.
#[test]
fn rendezvous_handoff_small_payload() {
    loom::model(|| {
        let (s, r) = kanal::bounded::<usize>(0);
        let sender = thread::spawn(move || {
            s.send(42).unwrap();
        });
        assert_eq!(r.recv().unwrap(), 42);
        sender.join().unwrap();
    });
}

/// Zero-capacity rendezvous handoff of a payload bigger than a pointer:
/// the value is moved through the counterpart's stack slot.
#[test]
fn rendezvous_handoff_big_payload() {
    loom::model(|| {
        let (s, r) = kanal::bounded::<Big>(0);
        let sender = thread::spawn(move || {
            s.send(Big::new(7)).unwrap();
        });
        assert_eq!(r.recv().unwrap(), Big::new(7));
        sender.join().unwrap();
    });
}

/// Two rendezvous sends from one sender must arrive in order.
#[test]
fn rendezvous_two_messages_in_order() {
    loom::model(|| {
        let (s, r) = kanal::bounded::<usize>(0);
        let sender = thread::spawn(move || {
            s.send(1).unwrap();
            s.send(2).unwrap();
        });
        assert_eq!(r.recv().unwrap(), 1);
        assert_eq!(r.recv().unwrap(), 2);
        sender.join().unwrap();
    });
}

/// Two concurrent rendezvous senders: both messages must arrive exactly
/// once, in either order.
#[test]
fn rendezvous_two_senders() {
    loom::model(|| {
        let (s, r) = kanal::bounded::<usize>(0);
        let s2 = s.clone();
        let a = thread::spawn(move || {
            s.send(1).unwrap();
        });
        let b = thread::spawn(move || {
            s2.send(2).unwrap();
        });
        let first = r.recv().unwrap();
        let second = r.recv().unwrap();
        assert_eq!(first + second, 3);
        assert_ne!(first, second);
        a.join().unwrap();
        b.join().unwrap();
    });
}

/// Bounded(1): the first message goes through the queue, the second
/// through a waiting send signal that the receiver completes; FIFO order
/// must hold across the queue/signal boundary.
#[test]
fn bounded_one_queue_then_signal() {
    loom::model(|| {
        let (s, r) = kanal::bounded::<usize>(1);
        let sender = thread::spawn(move || {
            s.send(1).unwrap();
            s.send(2).unwrap();
        });
        assert_eq!(r.recv().unwrap(), 1);
        assert_eq!(r.recv().unwrap(), 2);
        sender.join().unwrap();
    });
}

/// Two senders racing for the single queue slot of a bounded(1) channel
/// while the receiver drains it.
#[test]
fn bounded_one_two_senders() {
    loom::model(|| {
        let (s, r) = kanal::bounded::<usize>(1);
        let s2 = s.clone();
        let a = thread::spawn(move || {
            s.send(1).unwrap();
        });
        let b = thread::spawn(move || {
            s2.send(2).unwrap();
        });
        let first = r.recv().unwrap();
        let second = r.recv().unwrap();
        assert_eq!(first + second, 3);
        a.join().unwrap();
        b.join().unwrap();
    });
}

/// Unbounded sends never block; the receiver must still observe the
/// values in order and see the channel closed afterwards.
#[test]
fn unbounded_send_recv_close() {
    loom::model(|| {
        let (s, r) = kanal::unbounded::<usize>();
        let sender = thread::spawn(move || {
            s.send(1).unwrap();
            s.send(2).unwrap();
        });
        assert_eq!(r.recv().unwrap(), 1);
        assert_eq!(r.recv().unwrap(), 2);
        sender.join().unwrap();
        // sender dropped, queue drained: the channel is now closed
        assert!(r.recv().is_err());
    });
}

/// Sending into a channel whose receiver is concurrently dropped must
/// always fail, whether the drop lands before the send registers or
/// terminates the already parked sender.
#[test]
fn send_races_receiver_drop() {
    loom::model(|| {
        let (s, r) = kanal::bounded::<usize>(0);
        let sender = thread::spawn(move || {
            assert!(s.send(1).is_err());
        });
        drop(r);
        sender.join().unwrap();
    });
}

/// A queued message must survive the sender dropping: the receiver gets
/// the value no matter how the send/drop and recv interleave, and the
/// next receive reports the closed channel.
#[test]
fn recv_races_sender_drop() {
    loom::model(|| {
        let (s, r) = kanal::bounded::<usize>(1);
        let sender = thread::spawn(move || {
            s.send(9).unwrap();
        });
        assert_eq!(r.recv().unwrap(), 9);
        assert!(r.recv().is_err());
        sender.join().unwrap();
    });
}

/// try_recv takes the try_lock path; polling with yields must eventually
/// observe the value queued by the concurrent sender.
#[test]
fn try_recv_polls_concurrent_send() {
    loom::model(|| {
        let (s, r) = kanal::bounded::<usize>(1);
        let sender = thread::spawn(move || {
            s.send(3).unwrap();
        });
        let v = loop {
            if let Some(v) = r.try_recv().unwrap() {
                break v;
            }
            thread::yield_now();
        };
        assert_eq!(v, 3);
        sender.join().unwrap();
    });
}

/// send_many through a rendezvous channel: elements are handed to waiting
/// receivers via take_recvs or parked one signal at a time, and must come
/// out in order.
#[test]
fn send_many_rendezvous_in_order() {
    loom::model(|| {
        let (s, r) = kanal::bounded::<usize>(0);
        let sender = thread::spawn(move || {
            let mut buf: std::collections::VecDeque<usize> =
                [1, 2].into_iter().collect();
            s.send_many(&mut buf).unwrap();
            assert!(buf.is_empty());
        });
        assert_eq!(r.recv().unwrap(), 1);
        assert_eq!(r.recv().unwrap(), 2);
        sender.join().unwrap();
    });
}

/// drain_into_blocking collecting from two concurrent rendezvous senders:
/// the waitlist is taken in batches and every parked sender must be
/// completed exactly once after the lock is released.
#[test]
fn drain_into_blocking_two_senders() {
    loom::model(|| {
        let (s, r) = kanal::bounded::<usize>(0);
        let s2 = s.clone();
        let a = thread::spawn(move || {
            s.send(1).unwrap();
        });
        let b = thread::spawn(move || {
            s2.send(2).unwrap();
        });
        let mut buf = Vec::new();
        while buf.len() < 2 {
            r.drain_into_blocking(&mut buf).unwrap();
        }
        let sum: usize = buf.iter().sum();
        assert_eq!(sum, 3);
        assert_eq!(buf.len(), 2);
        a.join().unwrap();
        b.join().unwrap();
    });
}

#[cfg(feature = "async")]
mod async_models {
    use core::{future::Future, task::Context};

    use loom::{future::block_on, thread};

    /// Async rendezvous: sender and receiver both suspended futures on
    /// their own loom threads, woken through the AsyncSignal waker path.
    #[test]
    fn async_rendezvous() {
        loom::model(|| {
            let (s, r) = kanal::bounded_async::<usize>(0);
            let sender = thread::spawn(move || {
                block_on(s.send(5)).unwrap();
            });
            assert_eq!(block_on(r.recv()).unwrap(), 5);
            sender.join().unwrap();
        });
    }

    /// Sync sender delivering into an async receiver: exercises the
    /// tagged-pointer dispatch from a SyncSignal writer into an
    /// AsyncSignal waiter.
    #[test]
    fn sync_send_async_recv() {
        loom::model(|| {
            let (s, r) = kanal::bounded::<usize>(0);
            let r = r.to_async();
            let sender = thread::spawn(move || {
                s.send(11).unwrap();
            });
            assert_eq!(block_on(r.recv()).unwrap(), 11);
            sender.join().unwrap();
        });
    }

    /// Async sender delivering into a sync receiver: the opposite
    /// dispatch direction, waking the parked sync thread from a future.
    #[test]
    fn async_send_sync_recv() {
        loom::model(|| {
            let (s, r) = kanal::bounded::<usize>(0);
            let s = s.to_async();
            let sender = thread::spawn(move || {
                block_on(s.send(13)).unwrap();
            });
            assert_eq!(r.recv().unwrap(), 13);
            sender.join().unwrap();
        });
    }

    /// An async send racing the receiver drop must always error, whether
    /// it observes the closed channel up front or is terminated while
    /// suspended.
    #[test]
    fn async_send_races_receiver_drop() {
        loom::model(|| {
            let (s, r) = kanal::bounded_async::<usize>(0);
            let sender = thread::spawn(move || {
                assert!(block_on(s.send(1)).is_err());
            });
            drop(r);
            sender.join().unwrap();
        });
    }

    /// Async send_many suspending on a rendezvous channel: exercises the
    /// SendManyFuture path that reuses one finished signal per element via
    /// reset_send and re-registers it in the waitlist.
    #[test]
    fn async_send_many_rendezvous() {
        loom::model(|| {
            let (s, r) = kanal::bounded_async::<usize>(0);
            let sender = thread::spawn(move || {
                let mut buf: std::collections::VecDeque<usize> =
                    [1, 2].into_iter().collect();
                block_on(s.send_many(&mut buf)).unwrap();
                assert!(buf.is_empty());
            });
            let r = r.as_sync();
            assert_eq!(r.recv().unwrap(), 1);
            assert_eq!(r.recv().unwrap(), 2);
            sender.join().unwrap();
        });
    }

    /// Async drain_into_blocking waking up for a sync sender's delivery
    /// and batching whatever else is available.
    #[test]
    fn async_drain_into_blocking() {
        loom::model(|| {
            let (s, r) = kanal::bounded_async::<usize>(0);
            let sender = thread::spawn(move || {
                s.as_sync().send(4).unwrap();
            });
            let mut buf = Vec::new();
            while buf.is_empty() {
                block_on(r.drain_into_blocking(&mut buf)).unwrap();
            }
            assert_eq!(buf, [4]);
            sender.join().unwrap();
        });
    }

    /// Drop a ReceiveFuture that has already registered itself in the
    /// waitlist while a sync sender is concurrently delivering: the drop
    /// must either cancel the signal in time or wait for the committed
    /// sender and discard the delivered value; the sender must never
    /// observe an inconsistent signal. This is the cancellation path that
    /// cannot be reached through block_on.
    #[test]
    fn async_recv_future_drop_races_sender() {
        loom::model(|| {
            let (s, r) = kanal::bounded_async::<usize>(0);
            let sender = thread::spawn(move || {
                let _ = s.as_sync().send(1);
            });
            {
                let mut fut = Box::pin(r.recv());
                let waker = futures::task::noop_waker();
                let mut cx = Context::from_waker(&waker);
                // Register the receive signal in the waitlist (or complete
                // immediately if the sender already registered).
                let _ = fut.as_mut().poll(&mut cx);
                // fut dropped here, racing the sender's delivery
            }
            drop(r);
            sender.join().unwrap();
        });
    }
}
