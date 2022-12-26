/// Separate module for backoff strategy.
/// The reason of seperating backoff to independent module is that with this approach it is easier
///  to test and compare different backoff solutions.
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
    // This number will be added to the calculate pseudo random to avoid short spins
    const OFFSET: usize = 1 << 6;
    spin_wait((random_u8() as usize).wrapping_add(OFFSET));
}

/// Generates a pseudo u8 random number using atomics with LCG like algorithm
/// This genearator is only suited for special use-case of yield_now, and not recommended for use anywhere else.
#[allow(dead_code)]
#[inline(always)]
fn random_u8() -> u8 {
    // SEED number is inited with Mathematiclly proven super unlucky number. (I'm kidding, don't file a bug report please)
    static SEED: AtomicU8 = AtomicU8::new(13);
    const MULTIPLIER: u8 = 223;
    // Increment the seed atomically
    let seed = SEED.fetch_add(1, Ordering::SeqCst);
    // Use a LCG like algorithm to generate a random number from the seed
    seed.wrapping_mul(MULTIPLIER)
}

/// Generates a pseudo u32 random number using atomics with LCG like algorithm same as random_u8
#[allow(dead_code)]
#[inline(always)]
fn random_u32() -> u32 {
    static SEED: AtomicU32 = AtomicU32::new(13);
    const MULTIPLIER: u32 = 1812433253;
    let seed = SEED.fetch_add(1, Ordering::SeqCst);
    seed.wrapping_mul(MULTIPLIER)
}

// Randomizes the input 25%
#[allow(dead_code)]
#[inline(always)]
pub fn randomize(d: usize) -> usize {
    d + random_u32() as usize % (d >> 2)
}
