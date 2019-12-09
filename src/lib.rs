#![doc(html_root_url = "https://docs.rs/loom/0.2.14")]
#![deny(missing_debug_implementations, missing_docs, rust_2018_idioms)]
#![cfg_attr(test, deny(warnings))]

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
//! scheduling  The test is guaranteed to fail.
//!
//! # Writing tests
//!
//! Test cases using loom must be fully determinstic. All sources of
//! non-determism must be via loom types. This allows loom to validate the test
//! case and control the scheduling.
//!
//! Tests must use the loom synchronization types, such as `Atomic*`, `Mutex`,
//! `Condvar`, `thread::spawn`, etc. When writing a concurrent program, the
//! `std` types should be used when running the program and the `loom` types
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

macro_rules! if_futures {
    ($($t:tt)*) => {
        cfg_if::cfg_if! {
            if #[cfg(feature = "futures")] {
                $($t)*
            }
        }
    }
}

macro_rules! dbg {
    ($($t:tt)*) => {
        $($t)*
    };
}

pub mod alloc;
pub mod cell;
pub mod model;
mod rt;
pub mod sync;
pub mod thread;

#[doc(inline)]
pub use crate::model::model;

if_futures! {
    pub mod future;
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
