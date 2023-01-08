#[cfg(not(miri))]
pub(crate) const MESSAGES: usize = 100000;
#[cfg(miri)]
pub const MESSAGES: usize = 1024;
pub(crate) const THREADS: usize = 8;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(PartialEq, Eq, Debug)]
pub(crate) struct Padded {
    pub a: bool,
    pub b: u8,
    pub c: u32,
}

#[derive(PartialEq, Eq, Debug)]
#[repr(C)]
pub(crate) struct PaddedReprC {
    pub a: bool,
    pub b: u8,
    pub c: u32,
}

pub(crate) struct DropTester {
    pub i: usize,
    pub dropped: bool,
    pub counter: Arc<AtomicUsize>,
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
    pub fn new(counter: Arc<AtomicUsize>, i: usize) -> Self {
        if i == 0 {
            panic!("don't initialize DropTester with 0");
        }
        Self {
            i,
            dropped: false,
            counter,
        }
    }
}
