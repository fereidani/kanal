mod common;

use common::*;
use futures_core::FusedStream;

use kanal::{bounded_async, unbounded_async, AsyncReceiver, AsyncSender, ReceiveError, SendError};

use std::sync::atomic::{AtomicUsize, Ordering};
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
async fn async_drop_test() {
    let counter = Arc::new(AtomicUsize::new(0));
    async_mpmc_dyn!(let counter=counter.clone(),DropTester::new(counter.clone(), 10), Some(1));
    assert_eq!(counter.load(Ordering::SeqCst), MESSAGES as usize);
}

#[tokio::test]
async fn async_drop_test_in_signal() {
    let (s, r) = new_async(Some(10));

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
    r.close();
    for c in list {
        c.await.unwrap();
    }
    assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
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
    r.close();
}

#[tokio::test]
async fn async_drop_test_in_queue() {
    let (s, r) = new_async(Some(10));

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
    r.close();
    assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
}

#[tokio::test]
async fn async_drop_test_in_unused_signal() {
    let (s, r) = new_async(Some(10));

    let counter = Arc::new(AtomicUsize::new(0));
    for _ in 0..10 {
        let counter = counter.clone();
        let _ = s.send(DropTester::new(counter, 1234));
    }
    r.close();
    assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
}

#[tokio::test]
async fn async_drop_test_send_to_closed() {
    let (s, r) = new_async(Some(10));
    r.close();
    let counter = Arc::new(AtomicUsize::new(0));
    for _ in 0..10 {
        let counter = counter.clone();
        let _ = s.send(DropTester::new(counter, 1234)).await;
    }
    assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
}

#[tokio::test]
async fn async_drop_test_send_to_half_closed() {
    let (s, r) = new_async(Some(10));
    drop(r);
    let counter = Arc::new(AtomicUsize::new(0));
    for _ in 0..10 {
        let counter = counter.clone();
        let _ = s.send(DropTester::new(counter, 1234)).await;
    }
    assert_eq!(counter.load(Ordering::SeqCst), 10_usize);
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
