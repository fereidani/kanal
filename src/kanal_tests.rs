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
        return Self {
            i,
            dropped: false,
            counter,
        };
    }
}

#[cfg(test)]
mod tests {
    use crate::kanal_tests::{DropTester, MESSAGES, THREADS};
    use crate::{bounded, unbounded, Receiver, Sender};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

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
    fn single_message() {
        let (tx, rx) = new(Some(1));
        tx.send(1).unwrap();
        assert_eq!(rx.recv().unwrap(), 1);
    }

    #[test]
    fn drop_test() {
        let counter = Arc::new(AtomicU64::new(0));
        mpmc_dyn!(DropTester::new(counter.clone(), 10), Some(1));
        assert_eq!(counter.load(Ordering::SeqCst), MESSAGES as u64);
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
    use crate::kanal_tests::{DropTester, MESSAGES, THREADS};
    use crate::{bounded_async, unbounded_async, AsyncReceiver, AsyncSender};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

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
    async fn async_drop_test() {
        let counter = Arc::new(AtomicU64::new(0));
        async_mpmc_dyn!(let counter=counter.clone(),DropTester::new(counter.clone(), 10), Some(1));
        assert_eq!(counter.load(Ordering::SeqCst), MESSAGES as u64);
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
