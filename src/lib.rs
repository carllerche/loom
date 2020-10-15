#![doc(html_root_url = "https://docs.rs/loom/0.3.6")]
#![deny(missing_debug_implementations, missing_docs, rust_2018_idioms)]
#![cfg_attr(loom_nightly, feature(track_caller))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Loom is a tool for testing concurrent programs.
//!
//! # Background
//!
//! Testing concurrent programs is challenging. The Rust memory model is relaxed
//! and permits a large number of possible behaviors. Loom provides a way to
//! deterministically explore the various possible execution permutations.
//!
//! Consider a simple example:
//!
//! ```no_run
//! use std::sync::Arc;
//! use std::sync::atomic::AtomicUsize;
//! use std::sync::atomic::Ordering::SeqCst;
//! use std::thread;
//!
//! # /*
//! #[test]
//! # */
//! fn test_concurrent_logic() {
//!     let v1 = Arc::new(AtomicUsize::new(0));
//!     let v2 = v1.clone();
//!
//!     thread::spawn(move || {
//!         v1.store(1, SeqCst);
//!     });
//!
//!     assert_eq!(0, v2.load(SeqCst));
//! }
//! ```
//!
//! This program is obviously incorrect, yet the test can easily pass.
//!
//! The problem is compounded when Rust's relaxed memory model is considered.
//!
//! Historically, the strategy for testing concurrent code has been to run tests
//! in loops and hope that an execution fails. Doing this is not reliable, and,
//! in the event an iteration should fail, debugging the cause is exceedingly
//! difficult.
//!
//! # Solution
//!
//! Loom fixes the problem by controlling the scheduling of each thread. Loom
//! also simulates the Rust memory model such that it attempts all possible
//! valid behaviors. For example, an atomic load may return an old value instead
//! of the newest.
//!
//! The above example can be rewritten as:
//!
//! ```no_run
//! use loom::sync::atomic::AtomicUsize;
//! use loom::thread;
//!
//! use std::sync::Arc;
//! use std::sync::atomic::Ordering::SeqCst;
//!
//! # /*
//! #[test]
//! # */
//! fn test_concurrent_logic() {
//!     loom::model(|| {
//!         let v1 = Arc::new(AtomicUsize::new(0));
//!         let v2 = v1.clone();
//!
//!         thread::spawn(move || {
//!             v1.store(1, SeqCst);
//!         });
//!
//!         assert_eq!(0, v2.load(SeqCst));
//!     });
//! }
//! ```
//!
//! Loom will run the closure many times, each time with a different thread
//! scheduling. The test is guaranteed to fail.
//!
//! # Writing tests
//!
//! Test cases using loom must be fully determinstic. All sources of
//! non-determism must be via loom types. This allows loom to validate the test
//! case and control the scheduling.
//!
//! Tests must use the loom synchronization types, such as `Atomic*`, `Mutex`,
//! `RwLock`, `Condvar`, `thread::spawn`, etc. When writing a concurrent program,
//! the `std` types should be used when running the program and the `loom` types
//! when running the test.
//!
//! One way to do this is via cfg flags.
//!
//! It is important to not include other sources of non-determism in tests, such
//! as random number generators or system calls.
//!
//! # Yielding
//!
//! Some concurrent algorithms assume a fair scheduler. For example, a spin lock
//! assumes that, at some point, another thread will make enough progress for
//! the lock to become available.
//!
//! This presents a challenge for loom as the scheduler is not fair. In such
//! cases, loops must include calls to `yield_now`. This tells loom that another
//! thread needs to be scheduled in order for the current one to make progress.
//!
//! # Dealing with combinatorial explosion
//!
//! The number of possible threads scheduling has factorial growth as the
//! program complexity increases. Loom deals with this by reducing the state
//! space. Equivalent executions are elimited. For example, if two threads
//! **read** from the same atomic variable, loom does not attempt another
//! execution given that the order in which two threads read from the same
//! atomic cannot impact the execution.
//!
//! # Additional reading
//!
//! For more usage details, see the [README]
//!
//! [README]: https://github.com/tokio-rs/loom

macro_rules! if_futures {
    ($($t:tt)*) => {
        cfg_if::cfg_if! {
            if #[cfg(feature = "futures")] {
                #[cfg_attr(docsrs, doc(cfg(feature = "futures")))]
                $($t)*
            }
        }
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! debug {
    ($($t:tt)*) => {
        if $crate::__debug_enabled() {
            println!($($t)*);
        }
    };
}

macro_rules! dbg {
    ($($t:tt)*) => {
        $($t)*
    };
}

#[macro_use]
mod rt;

pub mod alloc;
pub mod cell;
pub mod lazy_static;
pub mod model;
pub mod sync;
pub mod thread;

#[doc(inline)]
pub use crate::model::model;

if_futures! {
    pub mod future;
}

#[doc(hidden)]
pub fn __debug_enabled() -> bool {
    rt::execution(|e| e.log)
}

/// Mock version of `std::thread_local!`.
// This is defined *after* all other code in `loom`, since we use
// `scoped_thread_local!` internally, which uses the `std::thread_local!` macro
// without namespacing it. Defining this after all other `loom` modules
// prevents internal code from accidentally using the mock thread local instead
// of the real one.
#[macro_export]
macro_rules! thread_local {
    // empty (base case for the recursion)
    () => {};

    // process multiple declarations
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr; $($rest:tt)*) => (
        $crate::__thread_local_inner!($(#[$attr])* $vis $name, $t, $init);
        $crate::thread_local!($($rest)*);
    );

    // handle a single declaration
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr) => (
        $crate::__thread_local_inner!($(#[$attr])* $vis $name, $t, $init);
    );
}

/// Mock version of `lazy_static::lazy_static!`.
#[macro_export]
macro_rules! lazy_static {
    ($(#[$attr:meta])* static ref $N:ident : $T:ty = $e:expr; $($t:tt)*) => {
        // use `()` to explicitly forward the information about private items
        $crate::__lazy_static_internal!($(#[$attr])* () static ref $N : $T = $e; $($t)*);
    };
    ($(#[$attr:meta])* pub static ref $N:ident : $T:ty = $e:expr; $($t:tt)*) => {
        $crate::__lazy_static_internal!($(#[$attr])* (pub) static ref $N : $T = $e; $($t)*);
    };
    ($(#[$attr:meta])* pub ($($vis:tt)+) static ref $N:ident : $T:ty = $e:expr; $($t:tt)*) => {
        $crate::__lazy_static_internal!($(#[$attr])* (pub ($($vis)+)) static ref $N : $T = $e; $($t)*);
    };
    () => ()
}

#[macro_export]
#[doc(hidden)]
macro_rules! __thread_local_inner {
    ($(#[$attr:meta])* $vis:vis $name:ident, $t:ty, $init:expr) => {
        $(#[$attr])* $vis static $name: $crate::thread::LocalKey<$t> =
            $crate::thread::LocalKey {
                init: (|| { $init }) as fn() -> $t,
                _p: std::marker::PhantomData,
            };
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! __lazy_static_internal {
    // optional visibility restrictions are wrapped in `()` to allow for
    // explicitly passing otherwise implicit information about private items
    ($(#[$attr:meta])* ($($vis:tt)*) static ref $N:ident : $T:ty = $init:expr; $($t:tt)*) => {
        #[allow(missing_copy_implementations)]
        #[allow(non_camel_case_types)]
        #[allow(dead_code)]
        $(#[$attr])*
        $($vis)* struct $N {__private_field: ()}
        #[doc(hidden)]
        $($vis)* static $N: $N = $N {__private_field: ()};
        impl ::core::ops::Deref for $N {
            type Target = $T;
            // this and the two __ functions below should really also be #[track_caller]
            fn deref(&self) -> &$T {
                #[inline(always)]
                fn __static_ref_initialize() -> $T { $init }

                #[inline(always)]
                fn __stability() -> &'static $T {
                    static LAZY: $crate::lazy_static::Lazy<$T> =
                        $crate::lazy_static::Lazy {
                            init: __static_ref_initialize,
                            _p: std::marker::PhantomData,
                        };
                    LAZY.get()
                }
                __stability()
            }
        }
        $crate::lazy_static!($($t)*);
    };
    () => ()
}
