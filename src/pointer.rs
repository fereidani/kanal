use std::{cell::UnsafeCell, mem::MaybeUninit};

/// Kanal Pointer is a structure to move data efficiently between sync and async context.
/// This mod transfer data with two different ways between threads:
/// 1. When data size T is bigger than pointer size:
///     holds pointer to that data in another side stack, and copies memory from that pointer location
/// 2. When data size T is equal or less than pointer size:
///     serialize data itself in pointer address, with this action KanalPtr removes one unnecessary memory load operation and improves speed.
/// This structure is unsafe. KanalPtr should be pinned to memory location or be a member of pinned structure to work correctly.
pub(crate) struct KanalPtr<T>(UnsafeCell<MaybeUninit<*mut T>>);

impl<T> Default for KanalPtr<T> {
    fn default() -> Self {
        Self(MaybeUninit::uninit().into())
    }
}

impl<T> KanalPtr<T> {
    /// Creates a KanalPtr from mut reference without forgeting or taking ownership
    /// Creator side should take care of forgeting the object after move action is completed.
    #[inline(always)]
    pub(crate) fn new_from(addr: &mut T) -> Self {
        if std::mem::size_of::<T>() > std::mem::size_of::<*mut T>() {
            Self(MaybeUninit::new(addr as *mut T).into())
        } else {
            // Safety: addr is valid memory object
            Self(unsafe { store_as_kanal_ptr(addr).into() })
        }
    }
    /// Creates a KanalPtr from owned object, receiver or creator should take care of dropping data inside ptr.
    #[cfg(feature = "async")]
    #[inline(always)]
    pub(crate) fn new_owned(d: T) -> Self {
        if std::mem::size_of::<T>() > std::mem::size_of::<*mut T>() {
            panic!("bug: data can't be stored when size of T is bigger than pointer size");
        } else {
            // Safety: d is valid memory object
            let ret = Self(unsafe { store_as_kanal_ptr(&d).into() });
            std::mem::forget(d);
            ret
        }
    }
    /// Creates a KanalPtr only for write operation, so it does not load data inside addr in KanalPtr as it is unnecessary
    #[inline(always)]
    pub(crate) fn new_write_address_ptr(addr: *mut T) -> Self {
        if std::mem::size_of::<T>() > std::mem::size_of::<*mut T>() {
            Self(MaybeUninit::new(addr).into())
        } else {
            Self(MaybeUninit::uninit().into())
        }
    }
    /// Creates a KanalPtr without checking or transforming the pointer to correct KanalPtr format,
    /// Caller should take uf being sure that provided address is in correct KanalPtr format
    #[cfg(feature = "async")]
    #[inline(always)]
    pub(crate) fn new_unchecked(addr: *mut T) -> Self {
        Self(MaybeUninit::new(addr).into())
    }
    /// Reads data based on movement protocol of KanalPtr based on size of T
    #[inline(always)]
    pub(crate) unsafe fn read(&self) -> T {
        if std::mem::size_of::<T>() > std::mem::size_of::<*mut T>() {
            // Data is in actual pointer location
            std::ptr::read((*self.0.get()).assume_init())
        } else {
            // Data is serialized as pointer location, load it from pointer value instead
            restore_from_kanal_ptr(*self.0.get())
        }
    }
    /// Writes data based on movement protocol of KanalPtr based on size of T
    #[inline(always)]
    pub(crate) unsafe fn write(&self, d: T) {
        if std::mem::size_of::<T>() > std::mem::size_of::<*mut T>() {
            // Data cant be stored as pointer value, move it to pointer location
            std::ptr::write((*self.0.get()).assume_init(), d);
        } else {
            // Data size is less or equal to pointer size, serialize data as pointer address
            *self.0.get() = store_as_kanal_ptr(&d);
            std::mem::forget(d);
        }
    }
}

/// this function stores data inside ptr in correct protocol format of KanalPtr for T types that are smaller than pointer size
#[inline(always)]
unsafe fn store_as_kanal_ptr<T>(ptr: *const T) -> MaybeUninit<*mut T> {
    let mut ret = MaybeUninit::uninit();
    if std::mem::size_of::<T>() == 0 {
        return ret;
    }
    std::ptr::copy_nonoverlapping(ptr, ret.as_mut_ptr() as *mut T, 1);
    ret
}

/// this function restores data inside ptr in correct protocol format of KanalPtr for T types that are smaller than pointer size
#[inline(always)]
unsafe fn restore_from_kanal_ptr<T>(ptr: MaybeUninit<*mut T>) -> T {
    if std::mem::size_of::<T>() == 0 {
        return std::mem::zeroed();
    }
    std::ptr::read(ptr.as_ptr() as *const T)
}
