#[cfg(feature = "async")]
mod utils;
#[cfg(feature = "async")]
mod asyncs {
    use crate::utils::*;
    use futures_core::FusedStream;
    use kanal::{
        bounded_async, unbounded_async, AsyncReceiver, AsyncSender, ReceiveError, SendError,
    };
    use std::{
        collections::VecDeque,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };

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

    async fn drain_into_blocking(channel_size: Option<usize>) {
        let (s, r) = new(channel_size);
        tokio::spawn(async move {
            let mut msgs = (0..MESSAGES).collect::<VecDeque<usize>>();
            s.send_many(&mut msgs).await.unwrap();
        });

        let mut vec = Vec::new();
        let mut total = 0;
        while total < MESSAGES {
            let count = r.drain_into_blocking(&mut vec).await.unwrap();
            assert!(count > 0);
            total += count;
        }
        assert_eq!(vec.len(), MESSAGES);
        for (i, v) in vec.iter().enumerate() {
            assert_eq!(*v, i);
        }
    }

    #[tokio::test]
    async fn drain_into_blocking_0() {
        drain_into_blocking(Some(0)).await;
    }

    #[tokio::test]
    async fn drain_into_blocking_1() {
        drain_into_blocking(Some(1)).await;
    }

    #[tokio::test]
    async fn drain_into_blocking_u() {
        drain_into_blocking(None).await;
    }

    // Test that drain_into_blocking actually awaits when no data is available
    async fn drain_into_blocking_waits(channel_size: Option<usize>) {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let (s, r) = new(channel_size);
        let received = Arc::new(AtomicBool::new(false));
        let received_clone = received.clone();

        // Spawn receiver task that will await for data
        let handle = tokio::spawn(async move {
            let mut vec = Vec::new();
            let count = r.drain_into_blocking(&mut vec).await.unwrap();
            received_clone.store(true, Ordering::SeqCst);
            assert!(count > 0);
            vec
        });

        // Give receiver time to start awaiting
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Receiver should still be awaiting (no data sent yet)
        assert!(
            !received.load(Ordering::SeqCst),
            "receiver should be awaiting"
        );

        // Now send data
        s.send(42usize).await.unwrap();

        // Wait for receiver to complete
        let vec = handle.await.unwrap();
        assert!(
            received.load(Ordering::SeqCst),
            "receiver should have received"
        );
        assert_eq!(vec, vec![42]);
    }

    #[tokio::test]
    async fn drain_into_blocking_waits_0() {
        drain_into_blocking_waits(Some(0)).await;
    }

    #[tokio::test]
    async fn drain_into_blocking_waits_1() {
        drain_into_blocking_waits(Some(1)).await;
    }

    #[tokio::test]
    async fn drain_into_blocking_waits_u() {
        drain_into_blocking_waits(None).await;
    }

    // Test drain_into_blocking with multiple concurrent senders
    async fn drain_into_blocking_mpsc(channel_size: Option<usize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        const NUM_SENDERS: usize = 10;
        const MSGS_PER_SENDER: usize = 100;
        const TOTAL_MSGS: usize = NUM_SENDERS * MSGS_PER_SENDER;

        let (s, r) = new(channel_size);
        let sent_count = Arc::new(AtomicUsize::new(0));

        // Spawn multiple sender tasks
        let handles: Vec<_> = (0..NUM_SENDERS)
            .map(|sender_id| {
                let s = s.clone();
                let sent_count = sent_count.clone();
                tokio::spawn(async move {
                    for i in 0..MSGS_PER_SENDER {
                        s.send(sender_id * MSGS_PER_SENDER + i).await.unwrap();
                        sent_count.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();

        drop(s); // Drop original sender

        // Receiver uses drain_into_blocking to collect all messages
        let mut vec = Vec::new();
        loop {
            match r.drain_into_blocking(&mut vec).await {
                Ok(count) => {
                    assert!(count > 0);
                    if vec.len() >= TOTAL_MSGS {
                        break;
                    }
                }
                Err(_) => break, // Channel closed
            }
        }

        // Wait for all senders to complete
        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(vec.len(), TOTAL_MSGS);

        // Verify all messages received (order may vary due to concurrency)
        vec.sort();
        for (i, v) in vec.iter().enumerate() {
            assert_eq!(*v, i);
        }
    }

    #[tokio::test]
    async fn drain_into_blocking_mpsc_0() {
        drain_into_blocking_mpsc(Some(0)).await;
    }

    #[tokio::test]
    async fn drain_into_blocking_mpsc_1() {
        drain_into_blocking_mpsc(Some(1)).await;
    }

    #[tokio::test]
    async fn drain_into_blocking_mpsc_u() {
        drain_into_blocking_mpsc(None).await;
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
}
