/// Separate module for backoff strategy.
/// The reason of seperating backoff to independent module is that with this approach it is easier
///  to test and compare different backoff solutions.
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

/// Puts current thread to sleep for amount of duration.
#[inline(always)]
pub fn sleep(dur: Duration) {
    std::thread::sleep(dur)
}

/// Emits cpu instruction that signals the processor that it is in spin loop.
#[allow(dead_code)]
#[inline(always)]
pub fn spin_hint() {
    std::hint::spin_loop()
}

/// Std library yield now
#[allow(dead_code)]
#[inline(always)]
pub fn yield_now_std() {
    // uses libc's sched_yield on unix and SwitchToThread on windows
    std::thread::yield_now();
}

/// Spins in a loop for finite amount of time.
#[allow(dead_code)]
#[inline(always)]
pub fn spin_wait(count: usize) {
    for _ in 0..count {
        spin_hint();
    }
}

// Cooperatively gives up a random timeslice
#[allow(dead_code)]
#[inline(always)]
pub fn yield_now() {
    // This is max acceptable spins for yield
    // minus 1 turns it to 0b1+ pattern so it is possible to use it with & operations
    const FILTER: usize = (1 << 8) - 1;
    // This number will be added to the calculate pseudo random to avoid short spins
    const OFFSET: usize = 1 << 6;
    spin_wait((random() & FILTER).wrapping_add(OFFSET));
}

/// Generates a pseudo random number using atomics
#[allow(dead_code)]
#[inline(always)]
pub fn random() -> usize {
    // SEED number is inited with Mathematiclly proven super unlucky number. (I'm kidding, don't file a bug report please)
    static SEED: AtomicUsize = AtomicUsize::new(13);
    // Whole point of alternative solution is to randomize wait time to avoid collision between cpu cores competing to acquire lock,
    // if it does not use acquire release and use relaxed ordering, collided loads will randomize in the exact same time.
    let mut current = SEED.load(Ordering::Acquire);
    loop {
        // try to swap current value with next random number
        match SEED.compare_exchange(
            current,
            // Linear congruential generator
            current.wrapping_mul(1103515245).wrapping_add(12345),
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            // Successfully updated current value to next random number, so current value is owned by this thread and valid to use
            Ok(_) => return current,
            // Failed updating current value, try again with new value
            Err(new) => current = new,
        }
    }
}

// Randomizes the input 25%
#[allow(dead_code)]
#[inline(always)]
pub fn randomize(d: usize) -> usize {
    d + random() % (d >> 2)
}
