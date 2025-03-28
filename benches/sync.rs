use criterion::*;
use std::thread::available_parallelism;

const BENCH_MSG_COUNT: usize = 1 << 16;

macro_rules! run_bench {
    ($b:expr, $tx:expr, $rx:expr, $readers:expr, $writers:expr) => {
        use std::thread::spawn;
        $b.iter(|| {
            let mut handles = Vec::with_capacity($readers + $writers);
            for _ in 0..$readers {
                let rx = $rx.clone();
                handles.push(spawn(move || {
                    for _ in 0..BENCH_MSG_COUNT / $readers {
                        rx.recv().unwrap();
                    }
                }));
            }
            for _ in 0..$writers {
                let tx = $tx.clone();
                handles.push(spawn(move || {
                    for i in 0..BENCH_MSG_COUNT / $writers {
                        tx.send(i).unwrap();
                    }
                }));
            }
            for handle in handles {
                handle.join().unwrap();
            }
        })
    };
}

fn mpmc(c: &mut Criterion) {
    c.bench_function("sync::mpmc::b0", |b| {
        let (tx, rx) = kanal::bounded::<usize>(0);
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count, core_count);
    });
    c.bench_function("sync::mpmc::b0_contended", |b| {
        let (tx, rx) = kanal::bounded::<usize>(0);
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count * 64, core_count * 64);
    });
    c.bench_function("sync::mpmc::b1", |b| {
        let (tx, rx) = kanal::bounded::<usize>(1);
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count, core_count);
    });
    c.bench_function("sync::mpmc::bn", |b| {
        let (tx, rx) = kanal::unbounded();
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count, core_count);
    });
}

fn mpsc(c: &mut Criterion) {
    c.bench_function("sync::mpsc::b0", |b| {
        let (tx, rx) = kanal::bounded::<usize>(0);
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count, 1);
    });
    c.bench_function("sync::mpsc::b0_contended", |b| {
        let (tx, rx) = kanal::bounded::<usize>(0);
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count, 1);
    });
    c.bench_function("sync::mpsc::b1", |b| {
        let (tx, rx) = kanal::bounded::<usize>(1);
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count, 1);
    });
    c.bench_function("sync::mpsc::bn", |b| {
        let (tx, rx) = kanal::unbounded();
        let core_count = usize::from(available_parallelism().unwrap());
        run_bench!(b, tx, rx, core_count, 1);
    });
}

fn spsc(c: &mut Criterion) {
    c.bench_function("sync::spsc::b0", |b| {
        let (tx, rx) = kanal::bounded::<usize>(0);
        run_bench!(b, tx, rx, 1, 1);
    });
    c.bench_function("sync::spsc::b1", |b| {
        let (tx, rx) = kanal::bounded::<usize>(1);
        run_bench!(b, tx, rx, 1, 1);
    });
}
criterion_group!(sync_bench, mpmc, mpsc, spsc);
criterion_main!(sync_bench);
