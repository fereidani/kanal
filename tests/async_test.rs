#[cfg(feature = "async")]
mod utils;
#[cfg(feature = "async")]
mod asyncs {
    use std::{
        collections::VecDeque,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };

    use futures_core::FusedStream;
    use kanal::{
        bounded_async, unbounded_async, AsyncReceiver, AsyncSender,
        ReceiveError, SendError,
    };

    use crate::utils::*;

    fn new<T>(cap: Option<usize>) -> (AsyncSender<T>, AsyncReceiver<T>) {
        match cap {
            None => unbounded_async(),
            Some(cap) => bounded_async(cap),
        }
    }

    macro_rules! mpmc_dyn {
        ($pre:stmt,$new:expr,$cap:expr) => {
            let (tx, rx) = new($cap);
            let mut list = Vec::new();
            for _ in 0..THREADS {
                let tx = tx.clone();
                $pre
                let h = tokio::spawn(async move {
                    for _i in 0..MESSAGES / THREADS {
                        tx.send($new).await.unwrap();
                    }
                });
                list.push(h);
            }

            for _ in 0..THREADS {
                let rx = rx.clone();
                let h = tokio::spawn(async move {
                    for _i in 0..MESSAGES / THREADS {
                        rx.recv().await.unwrap();
                    }
                });
                list.push(h);
            }

            for h in list {
                h.await.unwrap();
            }
        };
    }

    macro_rules! integrity_test {
        ($zero:expr,$ones:expr) => {
            let (tx, rx) = new(Some(0));
            tokio::spawn(async move {
                for _ in 0..MESSAGES {
                    tx.send($zero).await.unwrap();
                    tx.send($ones).await.unwrap();
                }
            });
            for _ in 0..MESSAGES {
                assert_eq!(rx.recv().await.unwrap(), $zero);
                assert_eq!(rx.recv().await.unwrap(), $ones);
            }
            let (tx, rx) = new(Some(1));
            tokio::spawn(async move {
                for _ in 0..MESSAGES {
                    tx.send($zero).await.unwrap();
                    tx.send($ones).await.unwrap();
                }
            });
            for _ in 0..MESSAGES {
                assert_eq!(rx.recv().await.unwrap(), $zero);
                assert_eq!(rx.recv().await.unwrap(), $ones);
            }
            let (tx, rx) = new(None);
            tokio::spawn(async move {
                for _ in 0..MESSAGES {
                    tx.send($zero).await.unwrap();
                    tx.send($ones).await.unwrap();
                }
            });
            for _ in 0..MESSAGES {
                assert_eq!(rx.recv().await.unwrap(), $zero);
                assert_eq!(rx.recv().await.unwrap(), $ones);
            }
        };
    }

    async fn mpsc(cap: Option<usize>) {
        let (tx, rx) = new(cap);
        let mut list = Vec::new();

        for _ in 0..THREADS {
            let tx = tx.clone();
            let h = tokio::spawn(async move {
                for _i in 0..MESSAGES / THREADS {
                    tx.send(Box::new(1)).await.unwrap();
                }
            });
            list.push(h);
        }

        for _ in 0..MESSAGES {
            assert_eq!(rx.recv().await.unwrap(), Box::new(1));
        }

        for h in list {
            h.await.unwrap();
        }
    }

    async fn seq(cap: Option<usize>) {
        let (tx, rx) = new(cap);

        for _i in 0..MESSAGES {
            tx.send(Box::new(1)).await.unwrap();
        }

        for _ in 0..MESSAGES {
            assert_eq!(rx.recv().await.unwrap(), Box::new(1));
        }
    }

    async fn spsc(cap: Option<usize>) {
        let (tx, rx) = new(cap);

        tokio::spawn(async move {
            for _i in 0..MESSAGES {
                tx.send(Box::new(1)).await.unwrap();
            }
        });

        for _ in 0..MESSAGES {
            assert_eq!(rx.recv().await.unwrap(), Box::new(1));
        }
    }

    async fn mpmc(cap: Option<usize>) {
        let (tx, rx) = new(cap);
        let mut list = Vec::new();
        for _ in 0..THREADS {
            let tx = tx.clone();
            let h = tokio::spawn(async move {
                for _i in 0..MESSAGES / THREADS {
                    tx.send(Box::new(1)).await.unwrap();
                }
            });
            list.push(h);
        }

        for _ in 0..THREADS {
            let rx = rx.clone();
            let h = tokio::spawn(async move {
                for _i in 0..MESSAGES / THREADS {
                    rx.recv().await.unwrap();
                }
            });
            list.push(h);
        }

        for h in list {
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn integrity_u8() {
        integrity_test!(0u8, !0u8);
    }

    #[tokio::test]
    async fn integrity_u16() {
        integrity_test!(0u16, !0u16);
    }

    #[tokio::test]
    async fn integrity_u32() {
        integrity_test!(0u32, !0u32);
    }

    #[tokio::test]
    async fn integrity_usize() {
        integrity_test!(0u64, !0u64);
    }

    #[tokio::test]
    async fn integrity_big() {
        integrity_test!((0u64, 0u64, 0u64, 0u64), (!0u64, !0u64, !0u64, !0u64));
    }

    #[tokio::test]
    async fn integrity_string() {
        integrity_test!("", "not empty");
    }

    #[tokio::test]
    async fn integrity_padded_rust() {
        integrity_test!(
            Padded {
                a: false,
                b: 0x0,
                c: 0x0
            },
            Padded {
                a: true,
                b: 0xFF,
                c: 0xFFFFFFFF
            }
        );
    }

    #[tokio::test]
    async fn integrity_padded_c() {
        integrity_test!(
            PaddedReprC {
                a: false,
                b: 0x0,
                c: 0x0
            },
            PaddedReprC {
                a: true,
                b: 0xFF,
                c: 0xFFFFFFFF
            }
        );
    }

    #[tokio::test]
    async fn drop_test() {
        let counter = Arc::new(AtomicUsize::new(0));
        mpmc_dyn!(let counter=counter.clone(),DropTester::new(counter.clone(), 10), Some(1));
        assert_eq!(counter.load(Ordering::SeqCst), MESSAGES);
    }

    #[tokio::test]
    async fn drop_test_in_signal() {
        let (s, r) = new(Some(10));

        let counter = Arc::new(AtomicUsize::new(0));
        let mut list = Vec::new();
        for _ in 0..10 {
            let counter = counter.clone();
            let s = s.clone();
            let c = tokio::spawn(async move {
                let _ = s.send(DropTester::new(counter, 1234)).await;
            });
            list.push(c);
        }
        r.close().unwrap();
        for c in list {
            c.await.unwrap();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
    }

    // Channel logic tests
    #[tokio::test]
    async fn recv_from_half_closed_queue() {
        let (tx, rx) = new(Some(1));
        tx.send(Box::new(1)).await.unwrap();
        drop(tx);
        // it's ok to receive data from queue of half closed channel
        assert_eq!(rx.recv().await.unwrap(), Box::new(1));
    }

    #[tokio::test]
    async fn recv_from_half_closed_channel() {
        let (tx, rx) = new::<u64>(Some(1));
        drop(tx);
        assert_eq!(rx.recv().await.err().unwrap(), ReceiveError());
    }

    #[tokio::test]
    async fn recv_from_closed_channel() {
        let (tx, rx) = new::<u64>(Some(1));
        tx.close().unwrap();
        assert_eq!(rx.recv().await.err().unwrap(), ReceiveError());
    }

    #[tokio::test]
    async fn recv_from_closed_channel_queue() {
        let (tx, rx) = new(Some(1));
        tx.send(Box::new(1)).await.unwrap();
        tx.close().unwrap();
        // it's not possible to read data from queue of fully closed channel
        assert_eq!(rx.recv().await.err().unwrap(), ReceiveError());
    }

    #[tokio::test]
    async fn send_to_half_closed_channel() {
        let (tx, rx) = new(Some(1));
        drop(rx);
        assert!(matches!(
            tx.send(Box::new(1)).await.err().unwrap(),
            SendError(_)
        ));
    }

    #[tokio::test]
    async fn send_to_closed_channel() {
        let (tx, rx) = new(Some(1));
        rx.close().unwrap();
        assert!(matches!(
            tx.send(Box::new(1)).await.err().unwrap(),
            SendError(_)
        ));
    }

    // Drop tests
    #[tokio::test]
    async fn recv_abort_test() {
        let (_s, r) = new::<DropTester>(Some(10));

        let mut list = Vec::new();
        for _ in 0..10 {
            let r = r.clone();
            let c = tokio::spawn(async move {
                if r.recv().await.is_ok() {
                    panic!("should not be ok");
                }
            });
            list.push(c);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        for c in list {
            c.abort();
        }
        r.close().unwrap();
    }

    // Drop tests
    #[tokio::test]
    async fn send_abort_test() {
        let (s, r) = new::<DropTester>(Some(0));
        let counter = Arc::new(AtomicUsize::new(0));
        let mut list = Vec::new();
        for _ in 0..10 {
            let s = s.clone();
            let counter = counter.clone();
            let c = tokio::spawn(async move {
                if s.send(DropTester::new(counter, 1234)).await.is_ok() {
                    panic!("should not be ok");
                }
            });
            list.push(c);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        for c in list {
            c.abort();
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
        r.close().unwrap();
    }

    #[tokio::test]
    async fn drop_test_in_queue() {
        let (s, r) = new(Some(10));

        let counter = Arc::new(AtomicUsize::new(0));
        let mut list = Vec::new();
        for _ in 0..10 {
            let counter = counter.clone();
            let s = s.clone();
            let c = tokio::spawn(async move {
                let _ = s.send(DropTester::new(counter, 1234)).await;
            });
            list.push(c);
        }
        for c in list {
            c.await.unwrap();
        }
        r.close().unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
    }

    #[tokio::test]
    async fn drop_test_in_unused_send_signal() {
        let (s, r) = new(Some(10));

        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..10 {
            let counter = counter.clone();
            drop(s.send(DropTester::new(counter, 1234)));
        }
        r.close().unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 10);
    }

    #[tokio::test]
    async fn drop_test_in_unused_recv_signal() {
        let (s, r) = new::<usize>(Some(10));

        for _ in 0..10 {
            drop(r.recv());
        }
        s.close().unwrap();
    }

    #[tokio::test]
    async fn drop_test_send_to_closed() {
        let (s, r) = new(Some(10));
        r.close().unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..10 {
            let counter = counter.clone();
            let _ = s.send(DropTester::new(counter, 1234)).await;
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
    }

    #[tokio::test]
    async fn drop_test_send_to_half_closed() {
        let (s, r) = new(Some(10));
        drop(r);
        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..10 {
            let counter = counter.clone();
            let _ = s.send(DropTester::new(counter, 1234)).await;
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
    }

    #[tokio::test]
    async fn vec_test() {
        mpmc_dyn!({}, vec![1, 2, 3], Some(1));
    }

    async fn send_many(channel_size: Option<usize>) {
        let (s, r) = new(channel_size);
        tokio::spawn(async move {
            let mut msgs = (0..MESSAGES).collect::<VecDeque<usize>>();
            s.send_many(&mut msgs).await.unwrap();
        });
        for i in 0..MESSAGES {
            assert_eq!(r.recv().await.unwrap(), i);
        }
    }

    #[tokio::test]
    async fn send_many_0() {
        send_many(Some(0)).await;
    }

    #[tokio::test]
    async fn send_many_1() {
        send_many(Some(1)).await;
    }

    #[tokio::test]
    async fn send_many_u() {
        send_many(None).await;
    }

    #[tokio::test]
    async fn one_msg() {
        let (s, r) = bounded_async::<u8>(1);
        s.send(0).await.unwrap();
        assert_eq!(r.recv().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn two_msg_0() {
        two_msg(0).await;
    }
    #[tokio::test]
    async fn two_msg_1() {
        two_msg(1).await;
    }
    #[tokio::test]
    async fn two_msg_2() {
        two_msg(2).await;
    }

    #[tokio::test]
    async fn mpsc_0() {
        mpsc(Some(0)).await;
    }

    #[tokio::test]
    async fn mpsc_n() {
        mpsc(Some(MESSAGES)).await;
    }

    #[tokio::test]
    async fn mpsc_u() {
        mpsc(None).await;
    }

    #[tokio::test]
    async fn mpmc_0() {
        mpmc(Some(0)).await;
    }

    #[tokio::test]
    async fn mpmc_n() {
        mpmc(Some(MESSAGES)).await;
    }

    #[tokio::test]
    async fn mpmc_u() {
        mpmc(None).await;
    }

    #[tokio::test]
    async fn spsc_0() {
        spsc(Some(0)).await;
    }

    #[tokio::test]
    async fn spsc_1() {
        spsc(Some(1)).await;
    }

    #[tokio::test]
    async fn spsc_n() {
        spsc(Some(MESSAGES)).await;
    }

    #[tokio::test]
    async fn spsc_u() {
        spsc(None).await;
    }

    #[tokio::test]
    async fn seq_n() {
        seq(Some(MESSAGES)).await;
    }

    #[tokio::test]
    async fn seq_u() {
        seq(None).await;
    }

    #[tokio::test]
    async fn stream() {
        use futures::stream::StreamExt;
        let (s, r) = new(Some(0));
        tokio::spawn(async move {
            for i in 0..MESSAGES {
                s.send(i).await.unwrap();
            }
        });
        let mut stream = r.stream();

        assert!(!stream.is_terminated());
        for i in 0..MESSAGES {
            assert_eq!(stream.next().await.unwrap(), i);
        }
        assert_eq!(stream.next().await, None);
        assert!(stream.is_terminated());
        assert_eq!(stream.next().await, None);
    }

    async fn two_msg(size: usize) {
        let (s, r) = bounded_async::<u8>(size);
        tokio::spawn(async move {
            s.send(0).await.unwrap();
            s.send(1).await.unwrap();
        });
        assert_eq!(r.recv().await.unwrap(), 0);
        assert_eq!(r.recv().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn spsc_overaligned_zst() {
        #[repr(align(1024))]
        struct Foo;

        let (tx, rx) = new(Some(0));

        tokio::spawn(async move {
            for _i in 0..MESSAGES {
                tx.send(Foo).await.unwrap();
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().await.unwrap();
        }
    }

    // Regression test for a spurious early-termination bug in the async receive
    // future's "signal already shared" fallback. When the future was re-polled
    // while pending with a changed waker and the peer had already taken the
    // signal out of the wait list, the code used to set the signal state to
    // Done before calling blocking_wait(). Because Done is greater than
    // LOCKED_STARVATION, blocking_wait() returned false immediately and the
    // receiver reported a spurious error. Under stream().buffer_unordered()
    // with a yield this race fires continuously, ending the stream early
    // and deadlocking the synchronous producer on the next rendezvous.
    //
    // The race is timing dependent and is reliably triggered only under release
    // optimizations (the original bug report was release-only); the test never
    // produces a false failure, so running it in debug is harmless even though
    // it may not reproduce the race there.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stream_buffer_unordered_rendezvous_integrity() {
        use futures::StreamExt;

        #[derive(Debug)]
        struct Payload {
            id: u64,
            values: Vec<u64>,
        }

        // Miri's isolated clock advances with executed work rather than real
        // time, so the native item count can never finish inside any
        // real-time timeout there (observed: ~750 items burn the whole 30s
        // budget while progressing normally). A reduced count still
        // exercises the same handoff interleavings under Miri's scheduler
        // and still catches the protocol UB this test guards against.
        #[cfg(not(miri))]
        const N: u64 = 10_000;
        #[cfg(miri)]
        const N: u64 = 128;

        let (async_sender, receiver) = bounded_async::<Payload>(0);
        // Drive the producer from a synchronous sender on a dedicated thread,
        // then drop the async sender so only the sync producer remains.
        let sender = async_sender.as_sync().clone();
        drop(async_sender);

        let producer = std::thread::spawn(move || {
            for id in 0..N {
                sender
                    .send(Payload {
                        id,
                        values: vec![id; 16],
                    })
                    .unwrap();
            }
        });

        // A regression either ends the stream early (fewer than N items) or
        // deadlocks the rendezvous; the timeout makes both fail fast instead of
        // hanging the test runner. Under Miri the budget is virtual time, so a
        // generous value costs nothing on a passing run.
        let timeout = Duration::from_secs(if cfg!(miri) { 600 } else { 30 });
        let count = tokio::time::timeout(timeout, async {
            receiver
                .stream()
                .map(|payload| async move {
                    tokio::task::yield_now().await;
                    assert!(
                        payload.values.iter().all(|value| *value == payload.id),
                        "corrupted payload: {payload:?}"
                    );
                })
                .buffer_unordered(2)
                .count()
                .await
        })
        .await
        .expect("stream did not finish within timeout (deadlock regression?)");

        assert_eq!(count, N as usize, "stream ended early; lost messages");

        producer.join().expect("producer thread panicked");
    }

    // drain_into_blocking on the async receiver returns a DrainIntoFuture:
    // it must never block the runtime thread, resolve with everything
    // available in one batch, and otherwise wait for the first delivery.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drain_into_blocking_batches_messages() {
        const COUNT: usize = if cfg!(miri) { 64 } else { 10_000 };
        let (s, r) = new::<usize>(Some(16));

        let producer = tokio::spawn(async move {
            for i in 0..COUNT {
                s.send(i).await.unwrap();
            }
        });

        let mut buf = Vec::new();
        while buf.len() < COUNT {
            assert!(r.drain_into_blocking(&mut buf).await.unwrap() >= 1);
        }
        assert_eq!(buf.len(), COUNT);
        for (i, v) in buf.iter().enumerate() {
            assert_eq!(*v, i);
        }
        producer.await.unwrap();
    }

    // On a rendezvous channel every message goes through the
    // registered-signal path of DrainIntoFuture plus its opportunistic
    // second batch pass.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drain_into_blocking_rendezvous() {
        const COUNT: usize = if cfg!(miri) { 64 } else { 1_000 };
        let (s, r) = new::<usize>(Some(0));

        let producer = tokio::spawn(async move {
            for i in 0..COUNT {
                s.send(i).await.unwrap();
            }
        });

        let mut buf = Vec::new();
        while buf.len() < COUNT {
            assert!(r.drain_into_blocking(&mut buf).await.unwrap() >= 1);
        }
        for (i, v) in buf.iter().enumerate() {
            assert_eq!(*v, i);
        }
        producer.await.unwrap();
    }

    // The shared non-blocking drain_into remains available on the async
    // receiver and never waits.
    #[tokio::test]
    async fn drain_into_non_blocking_on_async_receiver() {
        let (s, r) = new::<usize>(Some(8));
        let mut buf = Vec::new();
        // nothing available: returns immediately with zero
        assert_eq!(r.drain_into(&mut buf).unwrap(), 0);
        for i in 0..4 {
            s.send(i).await.unwrap();
        }
        assert_eq!(r.drain_into(&mut buf).unwrap(), 4);
        assert_eq!(buf, vec![0, 1, 2, 3]);
    }

    #[tokio::test]
    async fn drain_into_blocking_terminated_errors() {
        let (s, r) = new::<usize>(Some(4));
        s.send(0).await.unwrap();
        drop(s);
        let mut buf = Vec::new();
        // queued data is still drained after the sender is gone
        assert_eq!(r.drain_into_blocking(&mut buf).await.unwrap(), 1);
        // now the channel is terminated
        assert!(r.drain_into_blocking(&mut buf).await.is_err());
        assert_eq!(buf, vec![0]);
    }
}
