mod utils;
use std::time::Duration;
use utils::*;

// this test module uses the Box<usize> only to check for memory issues related
// to the drop impl with Miri, it's inefficient to use channels like this in
// production code.

fn delay() {
    std::thread::sleep(Duration::from_millis(10));
}

macro_rules! integrity_test {
    ($zero:expr,$ones:expr) => {
        let (tx, rx) = kanal::oneshot();
        crossbeam::scope(|scope| {
            scope.spawn(|_| {
                tx.send($zero).unwrap();
            });
            assert_eq!(rx.recv().unwrap(), $zero);
        })
        .unwrap();
        let (tx, rx) = kanal::oneshot();
        crossbeam::scope(|scope| {
            scope.spawn(|_| {
                tx.send($ones).unwrap();
            });
            assert_eq!(rx.recv().unwrap(), $ones);
        })
        .unwrap();
    };
}

#[test]
fn simple() {
    for _ in 0..100 {
        let (s, r) = kanal::oneshot();
        let h = std::thread::spawn(move || {
            s.send(Box::new(!0)).unwrap();
        });
        assert_eq!(Box::new(!0), r.recv().unwrap());
        h.join().unwrap();
    }
}

#[test]
fn receive_delayed() {
    for _ in 0..100 {
        let (s, r) = kanal::oneshot();
        let h = std::thread::spawn(move || {
            s.send(Box::new(!0)).unwrap();
        });
        delay();
        assert_eq!(Box::new(!0), r.recv().unwrap());
        h.join().unwrap();
    }
}

#[test]
fn send_delayed() {
    for _ in 0..100 {
        let (s, r) = kanal::oneshot();
        let h = std::thread::spawn(move || {
            delay();
            s.send(Box::new(!0)).unwrap();
        });
        assert_eq!(Box::new(!0), r.recv().unwrap());
        h.join().unwrap();
    }
}

#[test]
fn send_drop() {
    for _ in 0..100 {
        let (s, r) = kanal::oneshot();
        let h = std::thread::spawn(move || assert!(s.send(Box::new(!0)).is_err()));
        delay();
        drop(r);
        h.join().unwrap();
    }
}

#[test]
fn receive_drop() {
    for _ in 0..100 {
        let (s, r) = kanal::oneshot::<Box<usize>>();
        let h = std::thread::spawn(move || assert!(r.recv().is_err()));
        delay();
        drop(s);
        h.join().unwrap();
    }
}

#[test]
fn drop_both() {
    for _ in 0..100 {
        let (s, r) = kanal::oneshot::<Box<usize>>();
        let h = std::thread::spawn(move || drop(r));
        delay();
        drop(s);
        h.join().unwrap();
    }
}

#[test]
fn integrity_u8() {
    integrity_test!(0u8, !0u8);
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
fn integrity_box_u64() {
    integrity_test!(Box::new(0u64), Box::new(!0u64));
}

#[test]
fn integrity_big() {
    integrity_test!((0u64, 0u64, 0u64, 0u64), (!0u64, !0u64, !0u64, !0u64));
}

#[test]
fn integrity_string() {
    integrity_test!("", "not empty");
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

#[cfg(feature = "async")]
mod asyncs {
    use futures::{executor::block_on, Future};

    use super::*;
    use std::{task::Context, time::Duration};

    macro_rules! integrity_test {
        ($zero:expr,$ones:expr) => {
            let (tx, rx) = kanal::oneshot_async();
            tokio::spawn(async move {
                tx.send($zero).await.unwrap();
            });
            assert_eq!(rx.recv().await.unwrap(), $zero);
            let (tx, rx) = kanal::oneshot_async();
            tokio::spawn(async move {
                tx.send($ones).await.unwrap();
            });
            assert_eq!(rx.recv().await.unwrap(), $ones);
        };
    }

    async fn delay() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    #[tokio::test]
    async fn simple() {
        for _ in 0..100 {
            let (s, r) = kanal::oneshot_async();
            let h = tokio::spawn(async move {
                s.send(Box::new(!0)).await.unwrap();
            });
            assert_eq!(Box::new(!0), r.recv().await.unwrap());
            h.await.unwrap();
        }
    }
    #[tokio::test]
    async fn receive_delayed() {
        for _ in 0..100 {
            let (s, r) = kanal::oneshot_async();
            let h = tokio::spawn(async move {
                s.send(Box::new(!0)).await.unwrap();
            });
            delay().await;
            assert_eq!(Box::new(!0), r.recv().await.unwrap());
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn send_delayed() {
        for _ in 0..100 {
            let (s, r) = kanal::oneshot_async();
            let h = tokio::spawn(async move {
                delay().await;
                s.send(Box::new(!0)).await.unwrap();
            });
            assert_eq!(Box::new(!0), r.recv().await.unwrap());
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn send_to_dropped() {
        for _ in 0..100 {
            let (s, r) = kanal::oneshot_async();
            let h = tokio::spawn(async move {
                assert!(s.send(Box::new(!0)).await.is_err());
            });
            drop(r);
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn send_panic() {
        for _ in 0..100 {
            let (s, r) = kanal::oneshot_async();
            let h = tokio::spawn(async move {
                assert!(s.send(Box::new(!0)).await.is_err());
            });
            drop(r);
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn receive_from_dropped() {
        for _ in 0..100 {
            let (s, r) = kanal::oneshot_async::<Box<usize>>();
            let h = tokio::spawn(async move {
                assert!(r.recv().await.is_err());
            });
            drop(s);
            h.await.unwrap();
        }
    }

    #[test]
    fn receive_win_panic() {
        for _ in 0..100 {
            let (s, r) = kanal::oneshot_async::<Box<usize>>();
            let h = std::thread::spawn(move || {
                let mut fut = Box::pin(r.recv());
                let waker = ThreadWaker::new().into();
                let mut cx = Context::from_waker(&waker);
                assert!(fut.as_mut().poll(&mut cx).is_pending());
                panic!("oh no!");
            });
            std::thread::sleep(Duration::from_millis(10));
            block_on(s.send(10.into())).unwrap_err();
            h.join().unwrap_err();
        }
    }

    #[test]
    fn send_win_panic() {
        for _ in 0..100 {
            let (s, r) = kanal::oneshot_async::<Box<usize>>();
            let h = std::thread::spawn(move || {
                let mut fut = Box::pin(s.send(10.into()));
                let waker = ThreadWaker::new().into();
                let mut cx = Context::from_waker(&waker);
                assert!(fut.as_mut().poll(&mut cx).is_pending());
                panic!("oh no!");
            });
            std::thread::sleep(Duration::from_millis(10));
            block_on(r.recv()).unwrap_err();
            h.join().unwrap_err();
        }
    }

    #[tokio::test]
    async fn drop_both() {
        for _ in 0..100 {
            let (s, r) = kanal::oneshot_async::<Box<usize>>();
            let h = tokio::spawn(async move {
                drop(r);
            });
            drop(s);
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn integrity_u8() {
        integrity_test!(0u8, !0u8);
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
    async fn integrity_box_u64() {
        integrity_test!(Box::new(0u64), Box::new(!0u64));
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
    async fn integrity_big_tail() {
        integrity_test!((0u64, 0u64, 0u64, 0u8), (!0u64, !0u64, !0u64, !0u8));
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
}
