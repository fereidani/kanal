use criterion::*;
use std::thread::available_parallelism;
use tokio;

const BENCH_MSG_COUNT: usize = 1 << 16;

macro_rules! run_bench {
    ($b:expr, $tx:expr, $rx:expr, $readers:expr, $writers:expr) => {{
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(usize::from(available_parallelism().unwrap()))
            .enable_all()
            .build()
            .unwrap();
        $b.iter(|| {
            let mut handles = Vec::with_capacity($readers + $writers);
            for _ in 0..$readers {
                let rx = $rx.clone();
                handles.push(rt.spawn(async move {
                    for _ in 0..BENCH_MSG_COUNT / $readers {
                        rx.recv().await.unwrap();
                    }
                }));
            }
            for _ in 0..$writers {
                let tx = $tx.clone();
                handles.push(rt.spawn(async move {
                    for i in 0..BENCH_MSG_COUNT / $writers {
                        tx.send(i).await.unwrap();
                    }
                }));
            }
            for handle in handles {
                rt.block_on(handle).unwrap();
            }
        });
    }};
}

fn mpmc(c: &mut Criterion) {
    c.bench_function("async::mpmc::b0", |b| {
        let (tx, rx) = kanal::bounded_async::<usize>(0);
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count, core_count);
    });
    c.bench_function("async::mpmc::b0_contended", |b| {
        let (tx, rx) = kanal::bounded_async::<usize>(0);
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count * 64, core_count * 64);
    });
    c.bench_function("async::mpmc::b1", |b| {
        let (tx, rx) = kanal::bounded_async::<usize>(1);
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count, core_count);
    });
    c.bench_function("async::mpmc::bn", |b| {
        let (tx, rx) = kanal::unbounded_async();
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count, core_count);
    });
}

fn mpsc(c: &mut Criterion) {
    c.bench_function("async::mpsc::b0", |b| {
        let (tx, rx) = kanal::bounded_async::<usize>(0);
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count, 1);
    });
    c.bench_function("async::mpsc::b0_contended", |b| {
        let (tx, rx) = kanal::bounded_async::<usize>(0);
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count, 1);
    });
    c.bench_function("async::mpsc::b1", |b| {
        let (tx, rx) = kanal::bounded_async::<usize>(1);
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count, 1);
    });
    c.bench_function("async::mpsc::bn", |b| {
        let (tx, rx) = kanal::unbounded_async();
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count, 1);
    });
}

fn spsc(c: &mut Criterion) {
    c.bench_function("async::spsc::b0", |b| {
        let (tx, rx) = kanal::bounded_async::<usize>(0);
        run_bench!(b, tx, rx, 1, 1);
    });
    c.bench_function("async::spsc::b1", |b| {
        let (tx, rx) = kanal::bounded_async::<usize>(1);
        run_bench!(b, tx, rx, 1, 1);
    });
}
criterion_group!(async_bench, mpmc, mpsc, spsc);
criterion_main!(async_bench);
