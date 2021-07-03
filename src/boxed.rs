//! A pointer type for heap allocation.

use crate::alloc;
use std::{borrow, fmt, hash, mem, ptr};

/// A loom version of `std::boxed::Box` based on the leak tracking in `loom::alloc`.
///
/// # Known problems
///
/// This `Box` version can't be dereferenced in order to take the value
/// from the heap and bring it back to the stack. This is because `std::boxed::Box`
/// is tightly integrated with the compiler and uses magic that normal crates can't.
/// This version instead provides [`Box::into_value`] which does the same thing.
pub struct Box<T: ?Sized> {
    ptr: ptr::NonNull<T>,
}

impl<T> Box<T> {
    /// Allocates memory on the heap and then places `x` into it.
    pub fn new(x: T) -> Self {
        let layout = alloc::Layout::new::<T>();
        let ptr = unsafe { alloc::alloc(layout) } as *mut T;
        unsafe { ptr::write(ptr, x) };
        // SAFETY: `alloc::alloc` should never return a null pointer.
        Self {
            ptr: unsafe { ptr::NonNull::new_unchecked(ptr) },
        }
    }

    /// Consumes the box and returns the value in it.
    /// This is a a workaround. The standard library `Box` does not have this. Instead a
    /// a standard box can be dereferenced like `*std_box` to get the `T`. This can't be
    /// implemented outside of the standard library due to magic. so we need this workaround.
    ///
    /// In order to transparently switch between using loom and the standard library, consider
    /// introducing a function like this in your code and use it instead of directly
    /// dereferencing `Box`es:
    /// ```rust
    /// fn take<T>(b: Box<T>) -> T {
    ///     #[cfg(not(loom))]
    ///     {
    ///         *b
    ///     }
    ///     #[cfg(loom)]
    ///     {
    ///         b.into_value()
    ///     }
    /// }
    /// ```
    pub fn into_value(self) -> T {
        let value = unsafe { ptr::read(self.ptr.as_ptr()) };
        let layout = alloc::Layout::new::<T>();
        unsafe { alloc::dealloc(self.ptr.as_ptr() as *mut u8, layout) };
        mem::forget(self);
        value
    }
}

impl<T: ?Sized> Box<T> {
    /// Constructs a box from a raw pointer.
    ///
    /// After calling this function, the raw pointer is owned by the resulting Box. Specifically,
    /// the Box destructor will call the destructor of T and free the allocated memory.
    /// For this to be safe, the memory must have been allocated in accordance with the memory
    /// layout used by Box.
    ///
    /// # Safety
    ///
    /// This function is unsafe because improper use may lead to memory problems. For example,
    /// a double-free may occur if the function is called twice on the same raw pointer.
    #[inline]
    pub const unsafe fn from_raw(ptr: *mut T) -> Box<T> {
        Self {
            ptr: ptr::NonNull::new_unchecked(ptr),
        }
    }

    /// Consumes the Box, returning a wrapped raw pointer.
    ///
    /// The pointer will be properly aligned and non-null.
    ///
    /// After calling this function, the caller is responsible for the memory previously
    /// managed by the Box.
    #[inline]
    pub fn into_raw(b: Box<T>) -> *mut T {
        let ptr = b.ptr;
        mem::forget(b);
        ptr.as_ptr()
    }
}

impl<T: ?Sized> Drop for Box<T> {
    fn drop(&mut self) {
        unsafe {
            let size = mem::size_of_val(self.ptr.as_ref());
            let align = mem::align_of_val(self.ptr.as_ref());
            let layout = alloc::Layout::from_size_align(size, align).unwrap();
            ptr::drop_in_place(self.ptr.as_ptr());
            alloc::dealloc(self.ptr.as_ptr() as *mut u8, layout);
        }
    }
}

unsafe impl<T: Send> Send for Box<T> {}
unsafe impl<T: Sync> Sync for Box<T> {}

impl<T: ?Sized> std::ops::Deref for Box<T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: ?Sized> std::ops::DerefMut for Box<T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { self.ptr.as_mut() }
    }
}

impl<T: ?Sized> borrow::Borrow<T> for Box<T> {
    fn borrow(&self) -> &T {
        &**self
    }
}

impl<T: ?Sized> borrow::BorrowMut<T> for Box<T> {
    fn borrow_mut(&mut self) -> &mut T {
        &mut **self
    }
}

impl<T: ?Sized> AsRef<T> for Box<T> {
    fn as_ref(&self) -> &T {
        &**self
    }
}

impl<T: ?Sized> AsMut<T> for Box<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut **self
    }
}

impl<T: fmt::Display + ?Sized> fmt::Display for Box<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for Box<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: Clone> Clone for Box<T> {
    #[inline]
    fn clone(&self) -> Box<T> {
        Self::new(self.as_ref().clone())
    }
}

impl<T: ?Sized + PartialEq> PartialEq for Box<T> {
    #[inline]
    fn eq(&self, other: &Box<T>) -> bool {
        PartialEq::eq(&**self, &**other)
    }

    #[allow(clippy::partialeq_ne_impl)]
    #[inline]
    fn ne(&self, other: &Box<T>) -> bool {
        PartialEq::ne(&**self, &**other)
    }
}

impl<T: ?Sized + Eq> Eq for Box<T> {}

impl<T: ?Sized + PartialOrd> PartialOrd for Box<T> {
    #[inline]
    fn partial_cmp(&self, other: &Box<T>) -> Option<core::cmp::Ordering> {
        PartialOrd::partial_cmp(&**self, &**other)
    }
    #[inline]
    fn lt(&self, other: &Box<T>) -> bool {
        PartialOrd::lt(&**self, &**other)
    }
    #[inline]
    fn le(&self, other: &Box<T>) -> bool {
        PartialOrd::le(&**self, &**other)
    }
    #[inline]
    fn ge(&self, other: &Box<T>) -> bool {
        PartialOrd::ge(&**self, &**other)
    }
    #[inline]
    fn gt(&self, other: &Box<T>) -> bool {
        PartialOrd::gt(&**self, &**other)
    }
}

impl<T: ?Sized + Ord> Ord for Box<T> {
    #[inline]
    fn cmp(&self, other: &Box<T>) -> core::cmp::Ordering {
        Ord::cmp(&**self, &**other)
    }
}

impl<T: ?Sized + hash::Hash> hash::Hash for Box<T> {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        (**self).hash(state);
    }
}
