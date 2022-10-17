use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicPtr, Ordering};

/// A guard that executes a closure when being dropped
struct DropGuard<F: FnOnce()>(Option<F>);

impl<F> DropGuard<F>
    where
        F: FnOnce(),
{
    pub const fn new(func: F) -> Self {
        Self(Some(func))
    }
}

impl<F> Drop for DropGuard<F>
    where
        F: FnOnce(),
{
    fn drop(&mut self) {
        self.0.take().unwrap()()
    }
}

/// A synchronized cell, semantically equivalent to a `Send + Sync` version of `Cell<Option<T>>`,
/// that stores the value on the stack.
pub struct StackCell<T> {
    value: AtomicPtr<()>,
    // This is required to get rid of auto `Send` impl
    _marker: std::marker::PhantomData<*mut T>,
}

/// SAFETY:
/// `StackCell` does not have any thread local data, and is synchronized.
unsafe impl<T: Send> Send for StackCell<T> {}
/// SAFETY:
/// `StackCell` does not give out references to the stored value, and is itself synchronized.
unsafe impl<T> Sync for StackCell<T> {}

impl<T> StackCell<T> {
    /// Represents an empty cell
    const EMPTY: *mut () = std::ptr::null_mut();
    /// Represents a cell, that is currently exclusively read by someone (similar to &mut T)
    const PROCESSING: *mut () = !0usize as *mut _;

    /// Creates a new, empty cell
    pub const fn new() -> Self {
        Self {
            value: AtomicPtr::new(Self::EMPTY),
            _marker: std::marker::PhantomData,
        }
    }

    /// Attempts to place a value in the cell, and waits for its consumption.
    /// Drops the value, if it doesn't get consumed in time.
    ///
    /// `wait_for_store` is called while waiting for the cell to become empty.
    /// `wait_for_consume` is called while waiting for the value to be consumed.
    /// If one of the closures returns an error, the value is dropped (unless it was
    /// consumed in the meantime), and the function returns.
    #[inline]
    pub fn try_place<FS, FC>(
        &self,
        value: T,
        mut wait_for_store: FS,
        mut wait_for_consume: FC,
    ) -> Result<(), ()>
        where
            FS: FnMut() -> Result<(), ()>,
            FC: FnMut() -> Result<(), ()>,
    {
        // We store the `value` on our stack, and acquire a pointer to it's location.
        // We then wait for `self.value` to become free, and then store this pointer in `self.value`.
        // After that we wait for the value to be consumed. If the value does not get consumed in
        // time, we consume it ourselves and drop it.
        // It's essential for the soundness of this function, that we do **not** return until the
        // value was consumed by either someone else, or by us.

        let value = &mut MaybeUninit::new(value);
        let ptr = value.as_mut_ptr() as *mut ();

        let drop_value_guard = DropGuard::new(|| {
            // SAFETY:
            // We just stored a valid instance, and nobody got access to the value yet.
            // We also forget this guard as soon as someone else might have access to the value.
            unsafe { value.assume_init_drop() };
        });

        // try to store the value
        loop {
            let res = self.value.compare_exchange(
                Self::EMPTY,
                ptr,
                Ordering::Release,
                Ordering::Relaxed,
            );

            match res {
                Ok(_) => break,
                Err(_) => wait_for_store()?,
            }
        }

        // We cannot safely drop the value anymore, since it might have been read by someone else
        std::mem::forget(drop_value_guard);
        let take_value_guard = DropGuard::new(|| {
            // SAFETY:
            // The pointer points to a valid instance of T on our stack
            let _ = unsafe { self.take_if(ptr) };
        });

        // wait for the value to be consumed
        loop {
            match self.value.load(Ordering::Acquire) {
                Self::EMPTY => break,
                Self::PROCESSING => wait_for_consume()?,
                stored_ptr if stored_ptr == ptr => wait_for_consume()?,
                _ => break,
            }
        }

        // This is not required for soundness, but safes us a few unnecessary atomic operations
        std::mem::forget(take_value_guard);

        Ok(())
    }

    /// Takes the value out of the cell, if any is present
    #[inline]
    pub fn take(&self) -> Option<T> {
        let ptr = self.value.swap(Self::PROCESSING, Ordering::Acquire);
        let _guard = DropGuard::new(|| self.value.store(Self::EMPTY, Ordering::Release));

        match ptr {
            Self::EMPTY | Self::PROCESSING => None,
            // SAFETY:
            // The ptr is not EMPTY, so `try_place` provided a pointer to a valid value.
            // It's also not PROCESSING, so nobody is currently accessing the value.
            // This value is guaranteed to be valid until we set the ptr to EMPTY.
            _ => Some(unsafe { std::ptr::read(ptr as *mut T) }),
        }
    }

    /// SAFETY:
    /// `ptr` must point to a valid instance of `T` that is sound to read.
    #[inline]
    unsafe fn take_if(&self, ptr: *mut ()) -> Option<T> {
        let res = self.value.compare_exchange(ptr, Self::PROCESSING, Ordering::Acquire, Ordering::Relaxed);
        match res {
            // SAFETY:
            // The caller guarantees that the pointer is sound to read
            Ok(ptr) => {
                let _guard = DropGuard::new(|| self.value.store(Self::EMPTY, Ordering::Release));
                Some(unsafe { std::ptr::read(ptr as *mut T) })
            }
            Err(Self::PROCESSING) => loop {
                match self.value.load(Ordering::Acquire) {
                    // Shouldn't take long
                    Self::PROCESSING => std::hint::spin_loop(),
                    _ => break None,
                }
            },
            Err(_) => None,
        }
    }
}
