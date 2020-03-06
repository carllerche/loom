use crate::rt;

use std::cell::UnsafeCell;

/// CausalCell ensures access to the inner value are valid under the Rust memory
/// model.
#[derive(Debug)]
pub struct CausalCell<T> {
    data: UnsafeCell<T>,

    /// Causality associated with the cell
    obj: rt::CausalCell,
}

/// Deferred causal cell check
#[derive(Debug, Default)]
#[must_use]
pub struct CausalCheck(rt::CausalCheck);

impl<T> CausalCell<T> {
    /// Construct a new instance of `CausalCell` which will wrap the specified
    /// value.
    pub fn new(data: T) -> CausalCell<T> {
        CausalCell {
            data: UnsafeCell::new(data),
            obj: rt::CausalCell::new(),
        }
    }

    /// Get an immutable pointer to the wrapped value.
    ///
    /// # Panics
    ///
    /// This function will panic if the access is not valid under the Rust memory
    /// model.
    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(*const T) -> R,
    {
        self.obj.with(f, self.data.get())
    }

    /// Get an immutable pointer to the wrapped value, deferring the causality
    /// check.
    ///
    /// # Panics
    ///
    /// This function will panic if the access is not valid under the Rust memory
    /// model.
    pub fn with_deferred<F, R>(&self, f: F) -> (R, CausalCheck)
    where
        F: FnOnce(*const T) -> R,
    {
        let (res, check) = self.obj.with_deferred(f, self.data.get());
        (res, CausalCheck(check))
    }

    /// Get a mutable pointer to the wrapped value.
    ///
    /// # Panics
    ///
    /// This function will panic if the access is not valid under the Rust memory
    /// model.
    pub fn with_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(*mut T) -> R,
    {
        self.obj.with_mut(f, self.data.get())
    }

    /// Get a mutable pointer to the wrapped value.
    ///
    /// # Panics
    ///
    /// This function will panic if the access is not valid under the Rust memory
    /// model.
    pub fn with_deferred_mut<F, R>(&self, f: F) -> (R, CausalCheck)
    where
        F: FnOnce(*mut T) -> R,
    {
        let (res, check) = self.obj.with_deferred_mut(f, self.data.get());
        (res, CausalCheck(check))
    }

    /// Get an immutable pointer to the wrapped value.
    pub fn with_unchecked<F, R>(&self, f: F) -> R
    where
        F: FnOnce(*const T) -> R,
    {
        f(self.data.get())
    }

    /// Get a mutable pointer to the wrapped value.
    pub fn with_mut_unchecked<F, R>(&self, f: F) -> R
    where
        F: FnOnce(*mut T) -> R,
    {
        f(self.data.get())
    }

    /// Check that the current thread can make an immutable access without
    /// violating the Rust memory model.
    ///
    /// Specifically, this function checks that there is no concurrent mutable
    /// access with this immutable access, while allowing many concurrent
    /// immutable accesses.
    pub fn check(&self) {
        self.obj.check();
    }

    /// Check that the current thread can make a mutable access without violating
    /// the Rust memory model.
    ///
    /// Specifically, this function checks that there is no concurrent mutable
    /// access and no concurrent immutable access(es) with this mutable access.
    pub fn check_mut(&self) {
        self.obj.check_mut();
    }
}

impl CausalCheck {
    /// Panic if the CausalCell access was invalid.
    pub fn check(self) {
        self.0.check();
    }

    /// Merge this check with another check
    pub fn join(&mut self, other: CausalCheck) {
        self.0.join(other.0);
    }
}
