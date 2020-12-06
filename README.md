# Loom

Loom is a testing tool for concurrent Rust code. It runs a test many times,
permuting the possible concurrent executions of that test under the C11 memory
model. It uses state reduction techniques to avoid combinatorial explosion.

[![Build Status](https://dev.azure.com/tokio-rs/loom/_apis/build/status/tokio-rs.loom?branchName=master)](https://dev.azure.com/tokio-rs/loom/_build/latest?definitionId=2&branchName=master)

[Documentation](https://docs.rs/loom)

## Overview

Loom is an implementation of techniques described in [CDSChecker: Checking
Concurrent Data Structures Written with C/C++ Atomics][cdschecker]. It is a
library for writing unit tests where all possible thread interleavings are
checked. It also checks all possible atomic cell behaviors and validates
correct access to `UnsafeCell`.

[cdschecker]: http://demsky.eecs.uci.edu/publications/c11modelcheck.pdf

## Getting started

To use `loom`, first add this to your `Cargo.toml` (see below for more details):

```toml
[target.'cfg(loom)'.dependencies]
loom = "0.4"
```

Next, create a test file and add a test:

```rust
use loom::sync::Arc;
use loom::sync::atomic::AtomicUsize;
use loom::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use loom::thread;

#[test]
#[should_panic]
fn buggy_concurrent_inc() {
    loom::model(|| {
        let num = Arc::new(AtomicUsize::new(0));

        let ths: Vec<_> = (0..2)
            .map(|_| {
                let num = num.clone();
                thread::spawn(move || {
                    let curr = num.load(Acquire);
                    num.store(curr + 1, Release);
                })
            })
            .collect();

        for th in ths {
            th.join().unwrap();
        }

        assert_eq!(2, num.load(Relaxed));
    });
}
```

## Usage

Currently, using Loom comes with a bit of friction. Libraries must be written to
be Loom-aware, and doing so comes with some boilerplate. Over time, the friction
will be removed.

The following provides a brief overview of how to use Loom as part of the
testing workflow of a Rust crate.

### Structuring tests

When running Loom tests, the Loom concurrency types must be used in place of the
`std` types. However, when **not** running loom tests, the `std` should be used.
This means that library code will need to use conditional compilation to decide
which types to use.

It is recommended to use a `loom` cfg flag to signal using the Loom types. Then,
when running Loom tests, include `RUSTFLAGS="--cfg loom"` as part of the
command.

One strategy is to create module in your crate named `sync` or any other name of
your choosing. In this module, list out the types that need to be toggled
between Loom and `std`:

```rust
#[cfg(loom)]
pub(crate) use loom::sync::atomic::AtomicUsize;

#[cfg(not(loom))]
pub(crate) use std::sync::atomic::AtomicUsize;
```

Then, elsewhere in the library:

```rust
use crate::sync::AtomicUsize;
```

If you have any dependencies in your project which also use `loom`, you may wish to choose a cfg flag 
with a project specific flag to avoid conflicts with their use of loom. For 
example use `my_project_loom`, `#[cfg(my_project_loom)]` and 
`RUSTFLAGS="--cfg my_project_loom"`.

### Handling Loom API differences.

If your library must use Loom APIs that differ from `std` types, then the
library will be required to implement those APIs for `std`. For example, for
`UnsafeCell`, in the library's source, add the following:

```rust
#![cfg(not(loom))]

#[derive(Debug)]
pub(crate) struct UnsafeCell<T>(std::cell::UnsafeCell<T>);

impl<T> UnsafeCell<T> {
    pub(crate) fn new(data: T) -> UnsafeCell<T> {
        UnsafeCell(std::cell::UnsafeCell::new(data))
    }

    pub(crate) fn with<R>(&self, f: impl FnOnce(*const T) -> R) -> R {
        f(self.0.get())
    }

    pub(crate) fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R {
        f(self.0.get())
    }
}
```

### Running Loom tests

Loom tests must be run separately, with `RUSTFLAGS="--cfg loom"` specified. For
example, if the library includes a test file: `tests/loom_my_struct.rs` that
includes tests with `loom::model`, then run the following command:

```
RUSTFLAGS="--cfg loom" cargo test --test loom_my_struct
```

#### Handling large models

By default, Loom runs an **exhaustive** model. All possible execution paths are
checked. Loom's state reduction algorithms significantly reduce the state space
that must be explored, however, complex models can still take **significant**
time. There are two strategies to deal with this.

The first strategy is to run loom tests with `--release`. This will greatly
speed up execution time.

The second strategy is to **not** run an exhaustive check. Loom is able to set a
thread pre-emption bound. This means that Loom will check all possible
executions that include **at most** `n` thread pre-emptions. In practice,
setting the thread pre-emption bound to 2 or 3 is enough to catch most bugs.

To set the thread pre-emption bound, set the `LOOM_MAX_PREEMPTIONS` environment
variable when running tests. For example:

```
LOOM_MAX_PREEMPTIONS=3 RUSTFLAGS="--cfg loom" cargo test --test loom_my_struct
```

### Debugging failed tests

Loom's deterministic execution helps with debugging. The specific chain of
events leading to a test failure can be isolated.

When a loom test fails, the first step is to isolate the exact execution path
that resulted in the failure. To do this, Loom is able to output the execution
path to a file. Two environment variables are useful for this process:

- `LOOM_CHECKPOINT_FILE`
- `LOOM_CHECKPOINT_INTERVAL`

The first specifies the file to write to and read from. The second specifies how
often to write to the file. If the execution fails on the 10,000,000th
permutation, it is faster to write to a file every 10,000 iterations instead of
every single one.

To isolate the exact failing path, run the following commands:

```
LOOM_CHECKPOINT_FILE=my_test.json [other env vars] \
    cargo test --test loom_my_struct [failing test]
```

Then, the following:

```
LOOM_CHECKPOINT_INTERVAL=1 LOOM_CHECKPOINT_FILE=my_test.json [other env vars] \
    cargo test --test loom_my_struct [failing test]
```

The test should fail on the first permutation, effectively isolating the failure
scenario.

The next step is to enable additional log output. Again, there are some
environment variables for this:

- `LOOM_LOG`
- `LOOM_LOCATION`

The first environment variable, `LOOM_LOG`, outputs a marker on every thread switch.
This helps with tracing the exact steps in a threaded environment that results
in the test failure.

The second, `LOOM_LOCATION`, enables location tracking. This includes additional
information in panic messages that helps identify which specific field resulted
in the error.

Put together, the command becomes (yes, we know this is not great... but it
works):

```
LOOM_LOG=1 \
    LOOM_LOCATION=1 \
    LOOM_CHECKPOINT_INTERVAL=1 \
    LOOM_CHECKPOINT_FILE=my_test.json \
    RUSTFLAGS="--cfg loom" \
    [other env vars] \
    cargo test --test loom_my_struct [failing test]
```

## License

This project is licensed under the [MIT license](LICENSE).

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in `loom` by you, shall be licensed as MIT, without any additional
terms or conditions.
