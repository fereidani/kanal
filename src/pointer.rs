use core::{
    cell::UnsafeCell,
    mem::{forget, size_of, zeroed, MaybeUninit},
    ptr,
};

/// Kanal Pointer is a structure to move data efficiently between sync and async
/// context. This mod transfer data with two different ways between threads:
///
/// 1. When data size T is bigger than pointer size:
///
/// holds pointer to that data in another side stack, and copies memory from
/// that pointer location.
///
/// 2. When data size T is equal or less than pointer size:
///
/// serialize data itself in pointer address, with this action KanalPtr
/// removes one unnecessary memory load operation and improves speed. This
/// structure is unsafe. KanalPtr should be pinned to memory location or be a
/// member of pinned structure to work correctly. In particular, ZSTs should be
/// treated carefully as their alignments can be larger than the alignment of
/// pointers.
pub(crate) struct KanalPtr<T>(UnsafeCell<MaybeUninit<*mut T>>);

impl<T> KanalPtr<T> {
    /// Creates a KanalPtr from mut reference without forgetting or taking
    /// ownership Creator side should take care of forgetting the object
    /// after move action is completed.
    #[inline(always)]
    pub(crate) const fn new_from(addr: *mut T) -> Self {
        if size_of::<T>() > size_of::<*mut T>() {
            Self(UnsafeCell::new(MaybeUninit::new(addr)))
        } else {
            Self(UnsafeCell::new(unsafe { store_as_kanal_ptr(addr) }))
        }
    }
    /// Creates a KanalPtr only for write operation, so it does not load data
    /// inside addr in KanalPtr as it is unnecessary
    #[inline(always)]
    pub(crate) const fn new_write_address_ptr(addr: *mut T) -> Self {
        if size_of::<T>() > size_of::<*mut T>() {
            Self(UnsafeCell::new(MaybeUninit::new(addr)))
        } else {
            Self(UnsafeCell::new(MaybeUninit::uninit()))
        }
    }
    /// Reads data based on movement protocol of KanalPtr based on size of T
    #[inline(always)]
    pub(crate) const unsafe fn read(&self) -> T {
        if size_of::<T>() == 0 {
            zeroed()
        } else if size_of::<T>() > size_of::<*mut T>() {
            ptr::read((*self.0.get()).assume_init())
        } else {
            ptr::read((*self.0.get()).as_ptr() as *const T)
        }
    }
    /// Writes data based on movement protocol of KanalPtr based on size of T
    #[inline(always)]
    pub(crate) const unsafe fn write(&self, d: T) {
        if size_of::<T>() > size_of::<*mut T>() {
            ptr::write((*self.0.get()).assume_init(), d);
        } else {
            if size_of::<T>() > 0 {
                *self.0.get() = store_as_kanal_ptr(&d);
            }
            forget(d);
        }
    }
    /// Writes data based on movement protocol of KanalPtr based on size of T
    #[inline(always)]
    #[allow(unused)]
    pub(crate) const unsafe fn copy(&self, d: *const T) {
        if size_of::<T>() > size_of::<*mut T>() {
            // Data can't be stored as pointer value, move it to pointer
            // location
            ptr::copy_nonoverlapping(d, (*self.0.get()).assume_init(), 1);
        } else if size_of::<T>() > 0 {
            // Data size is less or equal to pointer size, serialize data as
            // pointer address
            *self.0.get() = store_as_kanal_ptr(d);
        }
    }
}

/// this function stores data inside ptr in correct protocol format of KanalPtr
/// for T types that are smaller than pointer size
#[inline(always)]
const unsafe fn store_as_kanal_ptr<T>(ptr: *const T) -> MaybeUninit<*mut T> {
    let mut ret = MaybeUninit::uninit();
    if size_of::<T>() > 0 {
        ptr::copy_nonoverlapping(ptr, ret.as_mut_ptr() as *mut T, 1);
    }
    ret
}
