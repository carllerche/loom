use crate::rt;

use std::ops;

/// Mock implementation of `std::sync::Arc`.
#[derive(Debug)]
pub struct Arc<T> {
    inner: std::sync::Arc<Inner<T>>,
}

#[derive(Debug)]
struct Inner<T> {
    // This must be the first field to make into_raw / from_raw work
    value: T,

    obj: rt::Arc,
}

impl<T> Arc<T> {
    /// Constructs a new `Arc<T>`.
    #[track_caller]
    pub fn new(value: T) -> Arc<T> {
        let inner = std::sync::Arc::new(Inner {
            value,
            obj: rt::Arc::new(location!()),
        });

        Arc { inner }
    }

    /// Gets the number of strong (`Arc`) pointers to this value.
    pub fn strong_count(this: &Self) -> usize {
        this.inner.obj.strong_count()
    }

    /// Returns a mutable reference to the inner value, if there are
    /// no other `Arc` pointers to the same value.
    pub fn get_mut(this: &mut Self) -> Option<&mut T> {
        if this.inner.obj.get_mut() {
            assert_eq!(1, std::sync::Arc::strong_count(&this.inner));
            Some(&mut std::sync::Arc::get_mut(&mut this.inner).unwrap().value)
        } else {
            None
        }
    }

    /// Returns `true` if the two `Arc`s point to the same value (not
    /// just values that compare as equal).
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&this.inner, &other.inner)
    }

    /// Consumes the `Arc`, returning the wrapped pointer.
    pub fn into_raw(this: Self) -> *const T {
        use std::mem;

        let ptr = &*this as *const _;
        mem::forget(this);
        ptr as *const T
    }

    /// Constructs an `Arc` from a raw pointer.
    pub unsafe fn from_raw(ptr: *const T) -> Self {
        let inner = std::sync::Arc::from_raw(ptr as *const Inner<T>);
        Arc { inner }
    }

    /// Returns the inner value, if the `Arc` has exactly one strong reference.
    pub fn try_unwrap(_this: Arc<T>) -> Result<T, Arc<T>> {
        unimplemented!();
    }
}

impl<T> ops::Deref for Arc<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner.value
    }
}

impl<T> Clone for Arc<T> {
    fn clone(&self) -> Arc<T> {
        self.inner.obj.ref_inc();

        Arc {
            inner: self.inner.clone(),
        }
    }
}

impl<T> Drop for Arc<T> {
    fn drop(&mut self) {
        if self.inner.obj.ref_dec() {
            assert_eq!(
                1,
                std::sync::Arc::strong_count(&self.inner),
                "something odd is going on"
            );
        }
    }
}

impl<T: Default> Default for Arc<T> {
    fn default() -> Arc<T> {
        Arc::new(Default::default())
    }
}

impl<T> From<T> for Arc<T> {
    fn from(t: T) -> Self {
        Arc::new(t)
    }
}
