use std::time::Duration;

// this test module uses the Box<usize> only to check for memory issues related to the drop impl with MIRI, it's inefficient to use a channel like this in production code.

fn delay() {
    std::thread::sleep(Duration::from_millis(10));
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

#[cfg(feature = "async")]
mod asyncs {
    use std::time::Duration;

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
    async fn send_drop() {
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
    async fn receive_drop() {
        for _ in 0..100 {
            let (s, r) = kanal::oneshot_async::<Box<usize>>();
            let h = tokio::spawn(async move {
                assert!(r.recv().await.is_err());
            });
            drop(s);
            h.await.unwrap();
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
}
