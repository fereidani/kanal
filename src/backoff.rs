/// Separate module for backoff strategy.
/// The reason of seperating backoff to independent module is that with
/// this approach it is easier  to test and compare different backoff
/// solutions.
use std::{
    sync::atomic::{AtomicU32, AtomicU8, Ordering},
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
    // This number will be added to the calculate pseudo random to avoid short
    // spins
    const OFFSET: usize = 1 << 6;
    spin_wait((random_u7() as usize).wrapping_add(OFFSET));
}

/// Generates a 7-bits pseudo random number using atomics with LCG like
/// algorithm This generator is only suited for special use-case of yield_now,
/// and not recommended for use anywhere else.
#[allow(dead_code)]
#[inline(always)]
fn random_u7() -> u8 {
    static SEED: AtomicU8 = AtomicU8::new(13);
    const MULTIPLIER: u8 = 113;
    // Increment the seed atomically, Relaxed ordering is enough as we need
    // atomic operation only on the SEED itself.
    let seed = SEED.fetch_add(1, Ordering::Relaxed);
    // Use a LCG like algorithm to generate a random number from the seed
    seed.wrapping_mul(MULTIPLIER) & 0x7F
}

/// Generates a pseudo u32 random number using atomics with LCG like algorithm
/// same as random_u8
#[allow(dead_code)]
#[inline(always)]
fn random_u32() -> u32 {
    static SEED: AtomicU32 = AtomicU32::new(13);
    const MULTIPLIER: u32 = 1812433253;
    let seed = SEED.fetch_add(1, Ordering::Relaxed);
    seed.wrapping_mul(MULTIPLIER)
}

// Randomizes the input 25%
#[allow(dead_code)]
#[inline(always)]
pub fn randomize(d: usize) -> usize {
    d - (d >> 3) + random_u32() as usize % (d >> 2)
}

// Spins until the condition becomes true
#[allow(dead_code)]
#[inline(always)]
pub fn spin_cond<F: Fn() -> bool>(cond: F) {
    const NO_YIELD: usize = 1;
    const SPIN_YIELD: usize = 1;
    const OS_YIELD: usize = 0;
    const ZERO_SLEEP: usize = 2;
    const SPINS: u32 = 8;

    let mut spins: u32 = SPINS;

    for _ in 0..NO_YIELD {
        for _ in 0..SPINS / 2 {
            if cond() {
                return;
            }
            spin_hint();
        }
    }

    loop {
        for _ in 0..SPIN_YIELD {
            yield_now();

            for _ in 0..spins {
                if cond() {
                    return;
                }
            }
        }

        for _ in 0..OS_YIELD {
            yield_now_std();

            for _ in 0..spins {
                if cond() {
                    return;
                }
            }
        }

        for _ in 0..ZERO_SLEEP {
            sleep(Duration::from_nanos(0));

            for _ in 0..spins {
                if cond() {
                    return;
                }
            }
        }

        if spins < (1 << 30) {
            spins <<= 1;
        }
        // Backoff 1ms
        sleep(Duration::from_nanos(1 << 20));
    }
}
