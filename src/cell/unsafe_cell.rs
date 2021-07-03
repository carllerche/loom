use crate::rt;

/// A checked version of `std::cell::UnsafeCell`.
///
/// Instead of providing a `get()` API, this version of `UnsafeCell` provides
/// `with` and `with_mut`. Both functions take a closure in order to track the
/// start and end of the access to the underlying cell.
#[derive(Debug)]
pub struct UnsafeCell<T: ?Sized> {
    /// Causality associated with the cell
    state: rt::Cell,
    data: std::cell::UnsafeCell<T>,
}

/// Token for tracking immutable access to the wrapped value.
///
/// This token must be held for as long as the access lasts.
#[derive(Debug)]
pub struct UnsafeCellRefToken<T: ?Sized> {
    data: *const T,
    token: rt::CellRefToken,
}

/// Token for tracking mutable access to the wrapped value.
///
/// This token must be held for as long as the access lasts.
#[derive(Debug)]
pub struct UnsafeCellMutToken<T: ?Sized> {
    data: *mut T,
    token: rt::CellMutToken,
}

impl<T> UnsafeCell<T> {
    /// Constructs a new instance of `UnsafeCell` which will wrap the specified value.
    #[track_caller]
    pub fn new(data: T) -> UnsafeCell<T> {
        let state = rt::Cell::new(location!());

        UnsafeCell {
            state,
            data: std::cell::UnsafeCell::new(data),
        }
    }
}

impl<T: ?Sized> UnsafeCell<T> {
    /// Get an immutable pointer to the wrapped value.
    ///
    /// # Panics
    ///
    /// This function will panic if the access is not valid under the Rust memory
    /// model.
    #[track_caller]
    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(*const T) -> R,
    {
        self.state
            .with(location!(), || f(self.data.get() as *const T))
    }

    /// Get a mutable pointer to the wrapped value.
    ///
    /// # Panics
    ///
    /// This function will panic if the access is not valid under the Rust memory
    /// model.
    #[track_caller]
    pub fn with_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(*mut T) -> R,
    {
        self.state.with_mut(location!(), || f(self.data.get()))
    }

    /// Get an `UnsafeCellRefToken` that provides scoped immutable access to the
    /// wrapped value.
    ///
    /// # Panics
    ///
    /// This function will panic if the access is not valid under the Rust memory
    /// model.
    pub unsafe fn guard_ref_access(&self) -> UnsafeCellRefToken<T> {
        let token = self.state.guard_ref_access(location!());
        UnsafeCellRefToken {
            data: self.data.get() as *const T,
            token,
        }
    }

    /// Get an `UnsafeCellMutToken` that provides scoped mutable access to the
    /// wrapped value.
    ///
    /// # Panics
    ///
    /// This function will panic if the access is not valid under the Rust memory
    /// model.
    pub unsafe fn guard_mut_access(&self) -> UnsafeCellMutToken<T> {
        let token = self.state.guard_mut_access(location!());
        UnsafeCellMutToken {
            data: self.data.get(),
            token,
        }
    }
}

impl<T: Default> Default for UnsafeCell<T> {
    fn default() -> UnsafeCell<T> {
        UnsafeCell::new(Default::default())
    }
}

impl<T> From<T> for UnsafeCell<T> {
    fn from(src: T) -> UnsafeCell<T> {
        UnsafeCell::new(src)
    }
}

impl<T: ?Sized> UnsafeCellRefToken<T> {
    /// Get an immutable pointer to the wrapped value.
    pub const fn get(&self) -> *const T {
        self.data
    }
}

impl<T: ?Sized> UnsafeCellMutToken<T> {
    /// Get a mutable pointer to the wrapped value.
    pub const fn get(&self) -> *mut T {
        self.data
    }
}
