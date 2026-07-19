//! Concurrency primitives used by the channel implementation.
//!
//! In normal builds this module re-exports the real std/core primitives.
//! When the crate is compiled with `RUSTFLAGS="--cfg loom"`, the loom
//! model-checked replacements are exported instead, so the channel protocol
//! can be exhaustively verified against the C11 memory model with
//! `loom::model`. See `tests/loom.rs` for the loom test suite.
//!
//! The exported [`UnsafeCell`] follows loom's closure-based accessor API
//! (`with`/`with_mut`) in both build modes; in normal builds the closures
//! compile down to plain pointer projections with no overhead.

#[cfg(not(loom))]
mod imp {
    // Depending on the feature set not every re-export is consumed (e.g.
    // std-mutex compiles the custom mutex and its AtomicBool out).
    #[allow(unused_imports)]
    pub(crate) use core::sync::atomic::{
        fence, AtomicBool, AtomicUsize, Ordering,
    };
    #[allow(unused_imports)]
    pub(crate) use std::thread::{
        current as current_thread, park, park_timeout, yield_now, Thread,
    };

    /// `UnsafeCell` facade matching loom's closure-based API.
    #[repr(transparent)]
    pub(crate) struct UnsafeCell<T>(core::cell::UnsafeCell<T>);

    impl<T> UnsafeCell<T> {
        #[inline(always)]
        pub(crate) const fn new(data: T) -> UnsafeCell<T> {
            UnsafeCell(core::cell::UnsafeCell::new(data))
        }
        /// Immutable access to the cell content. The caller must uphold the
        /// same aliasing rules as for [`core::cell::UnsafeCell::get`];
        /// under loom the access is additionally checked for data races.
        #[inline(always)]
        pub(crate) fn with<R>(&self, f: impl FnOnce(*const T) -> R) -> R {
            f(self.0.get())
        }
        /// Mutable access to the cell content. The caller must uphold the
        /// same aliasing rules as for [`core::cell::UnsafeCell::get`];
        /// under loom the access is additionally checked for data races.
        #[inline(always)]
        pub(crate) fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R {
            f(self.0.get())
        }
    }
}

#[cfg(loom)]
mod imp {
    pub(crate) use loom::{
        cell::UnsafeCell,
        sync::atomic::{fence, AtomicBool, AtomicUsize, Ordering},
        thread::{current as current_thread, park, yield_now, Thread},
    };
    // `park_timeout` is intentionally absent: loom does not model time.
    // The loom variants of the wait functions never time out; the
    // timeout-based public APIs are compiled but must not be called from
    // loom models.
}

pub(crate) use imp::*;
