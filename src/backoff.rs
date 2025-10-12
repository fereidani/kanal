/// This module provides various backoff strategies that can be used to reduce
/// the amount of busy waiting and improve the efficiency of concurrent systems.
///
/// The main idea behind separating backoff into an independent module is that
/// it makes it easier to test and compare different backoff solutions.
use core::{
    num::NonZeroUsize,
    sync::atomic::{AtomicU32, AtomicU8, AtomicUsize, Ordering},
    time::Duration,
};
use std::thread;

use branches::{likely, unlikely};

/// Puts the current thread to sleep for a specified duration.
#[inline(always)]
pub fn sleep(dur: Duration) {
    thread::sleep(dur)
}

/// Emits a CPU instruction that signals the processor that it is in a spin
/// loop.
#[allow(dead_code)]
#[inline(always)]
pub fn spin_hint() {
    std::hint::spin_loop()
}

/// Yields the thread to the scheduler.
#[allow(dead_code)]
#[inline(always)]
pub fn yield_os() {
    // On Unix systems, this function uses libc's sched_yield(), which cooperatively
    // gives up a random timeslice to another thread. On Windows systems, it
    // uses SwitchToThread(), which does the same thing.
    std::thread::yield_now();
}

/// Spins in a loop for a finite amount of time.
#[allow(dead_code)]
#[inline(always)]
pub fn spin_wait(count: usize) {
    for _ in 0..count {
        spin_hint();
    }
}

/// Yields the thread to the scheduler for a short random duration.
/// This function is implemented using a simple 7-bit pseudo random number
/// generator based on an atomic fetch-and-add operation.
#[allow(dead_code)]
#[inline(always)]
pub fn spin_rand() {
    // This number will be added to the calculated pseudo-random number to avoid
    // short spins.
    const OFFSET: usize = 1 << 6;
    spin_wait((random_u7() as usize).wrapping_add(OFFSET));
}

/// Generates a 7-bit pseudo-random number using an atomic fetch-and-add
/// operation and a linear congruential generator (LCG)-like algorithm.
/// This generator is only suited for the special use-case of yield_now(), and
/// not recommended for use anywhere else.
#[allow(dead_code)]
#[inline(always)]
fn random_u7() -> u8 {
    static SEED: AtomicU8 = AtomicU8::new(13);
    const MULTIPLIER: u8 = 113;
    // Increment the seed atomically. Relaxed ordering is enough as we only need an
    // atomic operation on the SEED itself.
    let seed = SEED.fetch_add(1, Ordering::Relaxed);
    // Use a LCG-like algorithm to generate a random number from the seed.
    seed.wrapping_mul(MULTIPLIER) & 0x7F
}

/// Generates a pseudo-random u32 number using an atomic fetch-and-add operation
/// and a LCG-like algorithm. This function is implemented using the same
/// algorithm as random_u8().
#[allow(dead_code)]
#[inline(always)]
fn random_u32() -> u32 {
    static SEED: AtomicU32 = AtomicU32::new(13);
    const MULTIPLIER: u32 = 1812433253;
    let seed = SEED.fetch_add(1, Ordering::Relaxed);
    seed.wrapping_mul(MULTIPLIER)
}

/// Randomizes the input by up to 25%.
/// This function is used to introduce some randomness into backoff strategies.
#[allow(dead_code)]
#[inline(always)]
pub fn randomize(d: usize) -> usize {
    d - (d >> 3) + random_u32() as usize % (d >> 2)
}

// Static atomic variable used to store the degree of parallelism.
// Initialized to 0, meaning that the parallelism degree has not been computed
// yet.
static PARALLELISM: AtomicUsize = AtomicUsize::new(0);

/// Retrieves the available degree of parallelism.
/// If the degree of parallelism has not been computed yet, it computes and
/// stores it in the PARALLELISM atomic variable. The degree of parallelism
/// typically corresponds to the number of processor cores that can execute
/// threads concurrently.
#[inline(always)]
pub fn get_parallelism() -> usize {
    let mut p = PARALLELISM.load(Ordering::Relaxed);
    // If the parallelism degree has not been computed yet.
    if p == 0 {
        // Try to get the degree of parallelism from available_parallelism.
        // If it is not available, default to 1.
        p = usize::from(thread::available_parallelism().unwrap_or(NonZeroUsize::new(1).unwrap()));
        PARALLELISM.store(p, Ordering::SeqCst);
    }
    // Return the computed degree of parallelism.
    p
}

/// Spins until the specified condition becomes true.
/// This function uses a combination of spinning, yielding, and sleeping to
/// reduce busy waiting and improve the efficiency of concurrent systems.
///
/// The function starts with a short spinning phase, followed by a longer
/// spinning and yielding phase, then a longer spinning and yielding phase with
/// the operating system's yield function, and finally a phase with zero-length
/// sleeping and yielding.
///
/// The function uses a geometric backoff strategy to increase the spin time
/// between each phase. The spin time starts at 8 iterations and doubles after
/// each unsuccessful iteration, up to a maximum of 2^30 iterations.
///
/// The function also uses a simple randomization strategy to introduce some
/// variation into the spin time.
///
/// The function takes a closure that returns a boolean value indicating whether
/// the condition has been met. The function returns when the condition is true.
#[allow(dead_code)]
#[allow(clippy::reversed_empty_ranges)]
#[inline(always)]
pub fn spin_cond<F: Fn() -> bool>(cond: F) {
    if get_parallelism() == 1 {
        // For environments with limited resources, such as small Virtual Private
        // Servers (VPS) or single-core systems, active spinning may lead to inefficient
        // CPU usage without performance benefits. This is due to the fact that there's
        // only one thread of execution, making it impossible for another thread to make
        // progress during the spin wait period.
        while unlikely(!cond()) {
            yield_os();
        }
        return;
    }

    const NO_YIELD: usize = 1;
    const SPIN_YIELD: usize = 1;
    const OS_YIELD: usize = 0;
    const ZERO_SLEEP: usize = 2;
    const SPINS: u32 = 8;
    let mut spins: u32 = SPINS;

    // Short spinning phase
    for _ in 0..NO_YIELD {
        for _ in 0..SPINS / 2 {
            if likely(cond()) {
                return;
            }
            spin_hint();
        }
    }

    // Longer spinning and yielding phase
    loop {
        for _ in 0..SPIN_YIELD {
            spin_rand();

            for _ in 0..spins {
                if likely(cond()) {
                    return;
                }
            }
        }

        // Longer spinning and yielding phase with OS yield
        for _ in 0..OS_YIELD {
            yield_os();

            for _ in 0..spins {
                if likely(cond()) {
                    return;
                }
            }
        }

        // Phase with zero-length sleeping and yielding
        for _ in 0..ZERO_SLEEP {
            sleep(Duration::from_nanos(0));

            for _ in 0..spins {
                if likely(cond()) {
                    return;
                }
            }
        }

        // Geometric backoff
        if spins < (1 << 30) {
            spins <<= 1;
        }
        // Backoff about 1ms
        sleep(Duration::from_nanos(1 << 20));
    }
}

macro_rules! return_if_some {
    ($result:expr) => {{
        let result = $result;
        if likely(result.is_some()) {
            return result;
        }
    }};
}

/// Computes a future timeout instant by adding a specified number of microseconds to the current time.
///
/// # Parameters
/// - `spin_micros`: The number of microseconds to add to the current time.
///
/// # Returns
/// A [`std::time::Instant`] indicating when the timeout will occur.
///
/// # Panics
/// This function will panic if the addition of the duration results in an overflow.
#[inline(always)]
#[allow(dead_code)]
pub(crate) fn spin_option_yield_only<T>(
    predicate: impl Fn() -> Option<T>,
    spin_micros: u64,
) -> Option<T> {
    // exit early if predicate is already satisfied
    return_if_some!(predicate());
    let timeout = std::time::Instant::now().checked_add(Duration::from_micros(spin_micros))?;
    loop {
        for _ in 0..32 {
            yield_os();
            return_if_some!(predicate());
        }
        if std::time::Instant::now() >= timeout {
            return None;
        }
    }
}
