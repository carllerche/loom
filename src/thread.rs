//! Mock implementation of `std::thread`.

use crate::rt;
pub use crate::rt::thread::AccessError;
pub use crate::rt::yield_now;

pub use std::thread::panicking;

use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::{fmt, io};

use tracing::trace;

/// Mock implementation of `std::thread::JoinHandle`.
pub struct JoinHandle<T> {
    result: Rc<RefCell<Option<std::thread::Result<T>>>>,
    notify: rt::Notify,
}

/// Mock implementation of `std::thread::LocalKey`.
pub struct LocalKey<T> {
    // Sadly, these fields have to be public, since function pointers in const
    // fns are unstable. When fn pointer arguments to const fns stabilize, these
    // should be made private and replaced with a `const fn new`.
    //
    // User code should not rely on the existence of these fields.
    #[doc(hidden)]
    pub init: fn() -> T,
    #[doc(hidden)]
    pub _p: PhantomData<fn(T)>,
}

/// Thread factory, which can be used in order to configure the properties of
/// a new thread.
#[derive(Debug)]
pub struct Builder {}

/// Mock implementation of `std::thread::spawn`.
pub fn spawn<F, T>(f: F) -> JoinHandle<T>
where
    F: FnOnce() -> T,
    F: 'static,
    T: 'static,
{
    let result = Rc::new(RefCell::new(None));
    let notify = rt::Notify::new(true, false);

    {
        let result = result.clone();
        rt::spawn(move || {
            *result.borrow_mut() = Some(Ok(f()));
            notify.notify();
        });
    }

    JoinHandle { result, notify }
}

impl Builder {
    /// Generates the base configuration for spawning a thread, from which
    /// configuration methods can be chained.
    pub fn new() -> Builder {
        Builder {}
    }

    /// Names the thread-to-be. Currently the name is used for identification
    /// only in panic messages.
    pub fn name(self, _name: String) -> Builder {
        self
    }

    /// Sets the size of the stack (in bytes) for the new thread.
    pub fn stack_size(self, _size: usize) -> Builder {
        self
    }

    /// Spawns a new thread by taking ownership of the `Builder`, and returns an
    /// `io::Result` to its `JoinHandle`.
    pub fn spawn<F, T>(self, f: F) -> io::Result<JoinHandle<T>>
    where
        F: FnOnce() -> T,
        F: Send + 'static,
        T: Send + 'static,
    {
        Ok(spawn(f))
    }
}

impl<T> JoinHandle<T> {
    /// Waits for the associated thread to finish.
    pub fn join(self) -> std::thread::Result<T> {
        self.notify.wait();
        self.result.borrow_mut().take().unwrap()
    }
}

impl<T: fmt::Debug> fmt::Debug for JoinHandle<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("JoinHandle").finish()
    }
}

impl<T: 'static> LocalKey<T> {
    /// Mock implementation of `std::thread::LocalKey::with`.
    pub fn with<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        self.try_with(f)
            .expect("cannot access a (mock) TLS value during or after it is destroyed")
    }

    /// Mock implementation of `std::thread::LocalKey::try_with`.
    pub fn try_with<F, R>(&'static self, f: F) -> Result<R, AccessError>
    where
        F: FnOnce(&T) -> R,
    {
        let value = match unsafe { self.get() } {
            Some(v) => v?,
            None => {
                // Init the value out of the `rt::execution`
                let value = (self.init)();

                rt::execution(|execution| {
                    trace!("LocalKey::try_with");

                    execution.threads.local_init(self, value);
                });

                unsafe { self.get() }.expect("bug")?
            }
        };
        Ok(f(value))
    }

    unsafe fn get(&'static self) -> Option<Result<&T, AccessError>> {
        unsafe fn transmute_lt<'a, 'b, T>(t: &'a T) -> &'b T {
            std::mem::transmute::<&'a T, &'b T>(t)
        }

        rt::execution(|execution| {
            trace!("LocalKey::get");

            let res = execution.threads.local(self)?;

            let local = match res {
                Ok(l) => l,
                Err(e) => return Some(Err(e)),
            };

            // This is, sadly, necessary to allow nested `with` blocks to access
            // different thread locals. The borrow on the thread-local needs to
            // escape the lifetime of the borrow on `execution`, since
            // `rt::execution` mutably borrows a RefCell, and borrowing it twice will
            // cause a panic. This should be safe, as we know the function being
            // passed the thread local will not outlive the thread on which
            // it's executing, by construction --- it's just kind of unfortunate.
            Some(Ok(transmute_lt(local)))
        })
    }
}

impl<T: 'static> fmt::Debug for LocalKey<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("LocalKey { .. }")
    }
}
