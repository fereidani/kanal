#![allow(dead_code)]
#[cfg(not(miri))]
const MESSAGES: usize = 1000000;
#[cfg(miri)]
const MESSAGES: usize = 1024;
const THREADS: usize = 64;

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

#[derive(PartialEq, Eq, Debug)]
struct Padded {
    a: bool,
    b: u8,
    c: u32,
}

#[derive(PartialEq, Eq, Debug)]
#[repr(C)]
struct PaddedReprC {
    a: bool,
    b: u8,
    c: u32,
}

struct DropTester {
    i: usize,
    dropped: bool,
    counter: Arc<AtomicU64>,
}

impl Drop for DropTester {
    fn drop(&mut self) {
        if self.dropped {
            panic!("double dropped");
        }
        if self.i == 0 {
            panic!("bug: i=0 is invalid value for drop tester");
        }
        self.dropped = true;
        self.counter.fetch_add(1, Ordering::SeqCst);
    }
}

impl DropTester {
    #[allow(dead_code)]
    fn new(counter: Arc<AtomicU64>, i: usize) -> Self {
        if i == 0 {
            panic!("don't initialize DropTester with 0");
        }
        Self {
            i,
            dropped: false,
            counter,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::kanal_tests::*;
    use crate::{bounded, unbounded, ReceiveError, Receiver, SendError, Sender};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    fn new<T>(cap: Option<usize>) -> (Sender<T>, Receiver<T>) {
        match cap {
            None => unbounded(),
            Some(cap) => bounded(cap),
        }
    }

    fn mpmc(cap: Option<usize>) {
        let (tx, rx) = new(cap);

        crossbeam::scope(|scope| {
            for _ in 0..THREADS {
                scope.spawn(|_| {
                    for _i in 0..MESSAGES / THREADS {
                        tx.send(1).unwrap();
                    }
                });
            }

            for _ in 0..THREADS {
                scope.spawn(|_| {
                    for _ in 0..MESSAGES / THREADS {
                        assert_eq!(rx.recv().unwrap(), 1);
                    }
                });
            }
        })
        .unwrap();
    }

    macro_rules! mpmc_dyn {
        ($new:expr,$cap:expr) => {
            let (tx, rx) = new($cap);

            crossbeam::scope(|scope| {
                for _ in 0..THREADS {
                    scope.spawn(|_| {
                        for _i in 0..MESSAGES / THREADS {
                            tx.send($new).unwrap();
                        }
                    });
                }

                for _ in 0..THREADS {
                    scope.spawn(|_| {
                        for _ in 0..MESSAGES / THREADS {
                            rx.recv().unwrap();
                        }
                    });
                }
            })
            .unwrap();
        };
    }

    macro_rules! integrity_test {
        ($zero:expr,$ones:expr) => {
            let (tx, rx) = new(Some(0));
            crossbeam::scope(|scope| {
                scope.spawn(|_| {
                    tx.send($zero).unwrap();
                    tx.send($ones).unwrap();
                });
                assert_eq!(rx.recv().unwrap(), $zero);
                assert_eq!(rx.recv().unwrap(), $ones);
            })
            .unwrap();
        };
    }

    fn mpsc(cap: Option<usize>) {
        let (tx, rx) = new(cap);

        crossbeam::scope(|scope| {
            for _ in 0..THREADS {
                scope.spawn(|_| {
                    for _i in 0..MESSAGES / THREADS {
                        tx.send(1).unwrap();
                    }
                });
            }

            for _ in 0..MESSAGES {
                assert_eq!(rx.recv().unwrap(), 1);
            }
        })
        .unwrap();
    }

    fn seq(cap: Option<usize>) {
        let (tx, rx) = new(cap);

        for _i in 0..MESSAGES {
            tx.send(1).unwrap();
        }

        for _ in 0..MESSAGES {
            assert_eq!(rx.recv().unwrap(), 1);
        }
    }

    fn spsc(cap: Option<usize>) {
        let (tx, rx) = new(cap);

        crossbeam::scope(|scope| {
            scope.spawn(|_| {
                for _i in 0..MESSAGES {
                    tx.send(1).unwrap();
                }
            });

            for _ in 0..MESSAGES {
                assert_eq!(rx.recv().unwrap(), 1);
            }
        })
        .unwrap();
    }

    #[test]
    fn integrity_u8() {
        integrity_test!(0u8, !0u8);
    }

    #[test]
    fn integrity_u16() {
        integrity_test!(0u16, !0u16);
    }

    #[test]
    fn integrity_u32() {
        integrity_test!(0u32, !0u32);
    }

    #[test]
    fn integrity_u64() {
        integrity_test!(0u64, !0u64);
    }

    #[test]
    fn integrity_big() {
        integrity_test!((0u64, 0u64, 0u64, 0u64), (!0u64, !0u64, !0u64, !0u64));
    }

    #[test]
    fn integrity_big_tail() {
        integrity_test!((0u64, 0u64, 0u64, 0u8), (!0u64, !0u64, !0u64, !0u8));
    }

    #[test]
    fn integrity_padded_rust() {
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

    #[test]
    fn integrity_padded_c() {
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

    #[test]
    fn single_message() {
        let (tx, rx) = new(Some(1));
        tx.send(1).unwrap();
        assert_eq!(rx.recv().unwrap(), 1);
    }

    // Channel logic tests
    #[test]
    fn recv_from_half_closed_queue() {
        let (tx, rx) = new(Some(1));
        tx.send(1).unwrap();
        drop(tx);
        // it's ok to receive data from queue of half closed channel
        assert_eq!(rx.recv().unwrap(), 1);
    }

    #[test]
    fn recv_from_half_closed_channel() {
        let (tx, rx) = new::<u64>(Some(1));
        drop(tx);
        assert_eq!(rx.recv().err().unwrap(), ReceiveError::SendClosed);
    }

    #[test]
    fn recv_from_closed_channel() {
        let (tx, rx) = new::<u64>(Some(1));
        tx.close();
        assert_eq!(rx.recv().err().unwrap(), ReceiveError::Closed);
    }

    #[test]
    fn recv_from_closed_channel_queue() {
        let (tx, rx) = new(Some(1));
        tx.send(1).unwrap();
        tx.close();
        // it's not possible to read data from queue of fully closed channel
        assert_eq!(rx.recv().err().unwrap(), ReceiveError::Closed);
    }

    #[test]
    fn send_to_half_closed_channel() {
        let (tx, rx) = new(Some(1));
        drop(rx);
        assert_eq!(tx.send(1).err().unwrap(), SendError::ReceiveClosed);
    }

    #[test]
    fn send_to_closed_channel() {
        let (tx, rx) = new(Some(1));
        rx.close();
        assert_eq!(tx.send(1).err().unwrap(), SendError::Closed);
    }

    // Channel drop tests
    #[test]
    fn drop_test() {
        let counter = Arc::new(AtomicU64::new(0));
        mpmc_dyn!(DropTester::new(counter.clone(), 10), Some(1));
        assert_eq!(counter.load(Ordering::SeqCst), MESSAGES as u64);
    }

    #[test]
    fn drop_test_in_queue() {
        let counter = Arc::new(AtomicU64::new(0));
        let (s, r) = new(Some(10));
        for _ in 0..10 {
            s.send(DropTester::new(counter.clone(), 1234)).unwrap();
        }
        r.close();
        assert_eq!(counter.load(Ordering::SeqCst), 10_u64);
    }

    #[test]
    fn drop_test_send_to_closed() {
        let counter = Arc::new(AtomicU64::new(0));
        let (s, r) = new(Some(10));
        r.close();
        for _ in 0..10 {
            // will fail
            let _ = s.send(DropTester::new(counter.clone(), 1234));
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10_u64);
    }

    #[test]
    fn drop_test_send_to_half_closed() {
        let counter = Arc::new(AtomicU64::new(0));
        let (s, r) = new(Some(10));
        drop(r);
        for _ in 0..10 {
            // will fail
            let _ = s.send(DropTester::new(counter.clone(), 1234));
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10_u64);
    }

    #[test]
    fn drop_test_in_signal() {
        let (s, r) = new(Some(0));

        crossbeam::scope(|scope| {
            let counter = Arc::new(AtomicU64::new(0));
            let mut list = Vec::new();
            for _ in 0..10 {
                let counter = counter.clone();
                let s = s.clone();
                let t = scope.spawn(move |_| {
                    let _ = s.send(DropTester::new(counter.clone(), 1234));
                });
                list.push(t);
            }
            std::thread::sleep(Duration::from_millis(1000));
            r.close();
            for t in list {
                t.join().unwrap();
            }
            assert_eq!(counter.load(Ordering::SeqCst), 10_u64);
        })
        .unwrap();
    }

    #[test]
    fn vec_test() {
        mpmc_dyn!(vec![1, 2, 3], Some(1));
    }

    #[test]
    fn spsc_1() {
        spsc(Some(1));
    }
    #[test]
    fn spsc_0() {
        spsc(Some(0));
    }
    #[test]
    fn spsc_n() {
        spsc(Some(MESSAGES));
    }
    #[test]
    fn spsc_u() {
        spsc(None);
    }

    #[test]
    fn mpsc_1() {
        mpsc(Some(1));
    }
    #[test]
    fn mpsc_0() {
        mpsc(Some(0));
    }
    #[test]
    fn mpsc_n() {
        mpsc(Some(MESSAGES));
    }
    #[test]
    fn mpsc_u() {
        mpsc(None);
    }

    #[test]
    fn mpmc_1() {
        mpmc(Some(1));
    }
    #[test]
    fn mpmc_0() {
        mpmc(Some(0));
    }
    #[test]
    fn mpmc_n() {
        mpmc(Some(MESSAGES));
    }
    #[test]
    fn mpmc_u() {
        mpmc(None);
    }
}
#[cfg(feature = "async")]
#[cfg(test)]
mod async_tests {
    use futures_core::FusedStream;

    use crate::kanal_tests::*;
    use crate::{
        bounded_async, unbounded_async, AsyncReceiver, AsyncSender, ReceiveError, SendError,
    };

    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    fn new_async<T>(cap: Option<usize>) -> (AsyncSender<T>, AsyncReceiver<T>) {
        match cap {
            None => unbounded_async(),
            Some(cap) => bounded_async(cap),
        }
    }

    macro_rules! async_mpmc_dyn {
        ($pre:stmt,$new:expr,$cap:expr) => {
            let (tx, rx) = new_async($cap);
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
            let (tx, rx) = new_async(Some(0));
            tokio::spawn(async move {
                tx.send($zero).await.unwrap();
                tx.send($ones).await.unwrap();
            });
            assert_eq!(rx.recv().await.unwrap(), $zero);
            assert_eq!(rx.recv().await.unwrap(), $ones);
        };
    }

    async fn async_mpsc(cap: Option<usize>) {
        let (tx, rx) = new_async(cap);
        let mut list = Vec::new();

        for _ in 0..THREADS {
            let tx = tx.clone();
            let h = tokio::spawn(async move {
                for _i in 0..MESSAGES / THREADS {
                    tx.send(1).await.unwrap();
                }
            });
            list.push(h);
        }

        for _ in 0..MESSAGES {
            assert_eq!(rx.recv().await.unwrap(), 1);
        }

        for h in list {
            h.await.unwrap();
        }
    }

    async fn async_seq(cap: Option<usize>) {
        let (tx, rx) = new_async(cap);

        for _i in 0..MESSAGES {
            tx.send(1).await.unwrap();
        }

        for _ in 0..MESSAGES {
            assert_eq!(rx.recv().await.unwrap(), 1);
        }
    }

    async fn async_spsc(cap: Option<usize>) {
        let (tx, rx) = new_async(cap);

        tokio::spawn(async move {
            for _i in 0..MESSAGES {
                tx.send(1).await.unwrap();
            }
        });

        for _ in 0..MESSAGES {
            assert_eq!(rx.recv().await.unwrap(), 1);
        }
    }

    async fn async_mpmc(cap: Option<usize>) {
        let (tx, rx) = new_async(cap);
        let mut list = Vec::new();
        for _ in 0..THREADS {
            let tx = tx.clone();
            let h = tokio::spawn(async move {
                for _i in 0..MESSAGES / THREADS {
                    tx.send(1).await.unwrap();
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
    async fn integrity_u64() {
        integrity_test!(0u64, !0u64);
    }

    #[tokio::test]
    async fn integrity_big() {
        integrity_test!((0u64, 0u64, 0u64, 0u64), (!0u64, !0u64, !0u64, !0u64));
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
    async fn async_drop_test() {
        let counter = Arc::new(AtomicU64::new(0));
        async_mpmc_dyn!(let counter=counter.clone(),DropTester::new(counter.clone(), 10), Some(1));
        assert_eq!(counter.load(Ordering::SeqCst), MESSAGES as u64);
    }

    #[tokio::test]
    async fn async_drop_test_in_signal() {
        let (s, r) = new_async(Some(10));

        let counter = Arc::new(AtomicU64::new(0));
        let mut list = Vec::new();
        for _ in 0..10 {
            let counter = counter.clone();
            let s = s.clone();
            let c = tokio::spawn(async move {
                let _ = s.send(DropTester::new(counter, 1234)).await;
            });
            list.push(c);
        }
        r.close();
        for c in list {
            c.await.unwrap();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10_u64);
    }

    // Channel logic tests
    #[tokio::test]
    async fn async_recv_from_half_closed_queue() {
        let (tx, rx) = new_async(Some(1));
        tx.send(1).await.unwrap();
        drop(tx);
        // it's ok to receive data from queue of half closed channel
        assert_eq!(rx.recv().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn async_recv_from_half_closed_channel() {
        let (tx, rx) = new_async::<u64>(Some(1));
        drop(tx);
        assert_eq!(rx.recv().await.err().unwrap(), ReceiveError::SendClosed);
    }

    #[tokio::test]
    async fn async_recv_from_closed_channel() {
        let (tx, rx) = new_async::<u64>(Some(1));
        tx.close();
        assert_eq!(rx.recv().await.err().unwrap(), ReceiveError::Closed);
    }

    #[tokio::test]
    async fn async_recv_from_closed_channel_queue() {
        let (tx, rx) = new_async(Some(1));
        tx.send(1).await.unwrap();
        tx.close();
        // it's not possible to read data from queue of fully closed channel
        assert_eq!(rx.recv().await.err().unwrap(), ReceiveError::Closed);
    }

    #[tokio::test]
    async fn async_send_to_half_closed_channel() {
        let (tx, rx) = new_async(Some(1));
        drop(rx);
        assert_eq!(tx.send(1).await.err().unwrap(), SendError::ReceiveClosed);
    }

    #[tokio::test]
    async fn async_send_to_closed_channel() {
        let (tx, rx) = new_async(Some(1));
        rx.close();
        assert_eq!(tx.send(1).await.err().unwrap(), SendError::Closed);
    }

    // Drop tests
    #[tokio::test]
    async fn async_recv_abort_test() {
        let (_s, r) = new_async::<DropTester>(Some(10));

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
        r.close();
    }

    // Drop tests
    #[tokio::test]
    async fn async_send_abort_test() {
        let (s, r) = new_async::<DropTester>(Some(0));
        let counter = Arc::new(AtomicU64::new(0));
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
        assert_eq!(counter.load(Ordering::SeqCst), 10_u64);
        r.close();
    }

    #[tokio::test]
    async fn async_drop_test_in_queue() {
        let (s, r) = new_async(Some(10));

        let counter = Arc::new(AtomicU64::new(0));
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
        r.close();
        assert_eq!(counter.load(Ordering::SeqCst), 10_u64);
    }

    #[tokio::test]
    async fn async_drop_test_in_unused_signal() {
        let (s, r) = new_async(Some(10));

        let counter = Arc::new(AtomicU64::new(0));
        for _ in 0..10 {
            let counter = counter.clone();
            let _ = s.send(DropTester::new(counter, 1234));
        }
        r.close();
        assert_eq!(counter.load(Ordering::SeqCst), 10_u64);
    }

    #[tokio::test]
    async fn async_drop_test_send_to_closed() {
        let (s, r) = new_async(Some(10));
        r.close();
        let counter = Arc::new(AtomicU64::new(0));
        for _ in 0..10 {
            let counter = counter.clone();
            let _ = s.send(DropTester::new(counter, 1234)).await;
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10_u64);
    }

    #[tokio::test]
    async fn async_drop_test_send_to_half_closed() {
        let (s, r) = new_async(Some(10));
        drop(r);
        let counter = Arc::new(AtomicU64::new(0));
        for _ in 0..10 {
            let counter = counter.clone();
            let _ = s.send(DropTester::new(counter, 1234)).await;
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10_u64);
    }

    #[tokio::test]
    async fn async_vec_test() {
        async_mpmc_dyn!({}, vec![1, 2, 3], Some(1));
    }

    #[tokio::test]
    async fn async_one_msg() {
        let (s, r) = bounded_async::<u8>(1);
        s.send(0).await.unwrap();
        assert_eq!(r.recv().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn async_two_msg_0() {
        async_two_msg(0).await;
    }
    #[tokio::test]
    async fn async_two_msg_1() {
        async_two_msg(1).await;
    }
    #[tokio::test]
    async fn async_two_msg_2() {
        async_two_msg(2).await;
    }

    #[tokio::test]
    async fn async_mpmc_0() {
        async_mpmc(Some(0)).await;
    }

    #[tokio::test]
    async fn async_mpmc_n() {
        async_mpmc(Some(MESSAGES)).await;
    }

    #[tokio::test]
    async fn async_mpmc_u() {
        async_spsc(None).await;
    }

    #[tokio::test]
    async fn async_spsc_0() {
        async_spsc(Some(0)).await;
    }

    #[tokio::test]
    async fn async_spsc_1() {
        async_spsc(Some(1)).await;
    }

    #[tokio::test]
    async fn async_spsc_n() {
        async_spsc(Some(MESSAGES)).await;
    }

    #[tokio::test]
    async fn async_spsc_u() {
        async_spsc(None).await;
    }

    #[tokio::test]
    async fn async_seq_n() {
        async_mpsc(Some(MESSAGES)).await;
    }

    #[tokio::test]
    async fn async_seq_u() {
        async_mpsc(None).await;
    }

    #[tokio::test]
    async fn async_stream() {
        use futures::stream::StreamExt;
        let (s, r) = new_async(Some(0));
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

    async fn async_two_msg(size: usize) {
        let (s, r) = bounded_async::<u8>(size);
        tokio::spawn(async move {
            s.send(0).await.unwrap();
            s.send(1).await.unwrap();
        });
        assert_eq!(r.recv().await.unwrap(), 0);
        assert_eq!(r.recv().await.unwrap(), 1);
    }
}
