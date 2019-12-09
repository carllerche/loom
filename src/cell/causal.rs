use crate::rt::{self, VersionVec};

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing::trace;

/// CausalCell ensures access to the inner value are valid under the Rust memory
/// model.
#[derive(Debug)]
pub struct CausalCell<T> {
    data: UnsafeCell<T>,

    /// Causality associated with the cell
    state: Arc<Mutex<State>>,
}

/// Deferred causal cell check
#[derive(Debug)]
#[must_use]
pub struct CausalCheck {
    deferred: Vec<(Arc<Mutex<State>>, usize)>,
}

#[derive(Debug)]
struct State {
    causality: Causality,
    deferred: HashMap<usize, Deferred>,
    next_index: usize,
}

#[derive(Debug)]
struct Deferred {
    /// True if a mutable access
    is_mut: bool,

    /// Thread causality at the point the access happened.
    thread_causality: VersionVec,

    /// Result
    result: Result<(), String>,
}

#[derive(Debug, Clone)]
struct Causality {
    // The transitive closure of all immutable accessses of `data`.
    immut_access_version: VersionVec,

    // The last mutable access of `data`.
    mut_access_version: VersionVec,
}

impl<T> CausalCell<T> {
    /// Construct a new instance of `CausalCell` which will wrap the specified
    /// value.
    pub fn new(data: T) -> CausalCell<T> {
        let v = rt::execution(|execution| {
            trace!("CausalCell::new");

            execution.threads.active().causality.clone()
        });

        CausalCell {
            data: UnsafeCell::new(data),
            state: Arc::new(Mutex::new(State {
                causality: Causality {
                    immut_access_version: v.clone(),
                    mut_access_version: v,
                },
                deferred: HashMap::new(),
                next_index: 0,
            })),
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
        rt::critical(|| {
            self.check();
            self.with_unchecked(f)
        })
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
        rt::critical(|| {
            rt::execution(|execution| {
                trace!("CausalCell::with_deferred");

                let thread_causality = &execution.threads.active().causality;

                let mut state = self.state.lock().unwrap();
                let index = state.next_index;
                let result = state.causality.check(thread_causality);

                state.deferred.insert(
                    index,
                    Deferred {
                        is_mut: false,
                        thread_causality: thread_causality.clone(),
                        result,
                    },
                );

                state.next_index += 1;

                let check = CausalCheck {
                    deferred: vec![(self.state.clone(), index)],
                };

                (self.with_unchecked(f), check)
            })
        })
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
        rt::critical(|| {
            self.check_mut();
            self.with_mut_unchecked(f)
        })
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
        rt::critical(|| {
            rt::execution(|execution| {
                trace!("CausalCell::with_deferred_mut");

                let thread_causality = &execution.threads.active().causality;

                let mut state = self.state.lock().unwrap();
                let index = state.next_index;
                let result = state.causality.check_mut(thread_causality);

                state.deferred.insert(
                    index,
                    Deferred {
                        is_mut: true,
                        thread_causality: thread_causality.clone(),
                        result,
                    },
                );

                state.next_index += 1;

                let check = CausalCheck {
                    deferred: vec![(self.state.clone(), index)],
                };

                (self.with_mut_unchecked(f), check)
            })
        })
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
        rt::execution(|execution| {
            trace!("CausalCell::check");

            let thread_causality = &execution.threads.active().causality;
            let mut state = self.state.lock().unwrap();

            state.causality.check(thread_causality).unwrap();
            state.causality.immut_access_version.join(thread_causality);

            for deferred in state.deferred.values_mut() {
                deferred.check(thread_causality);
            }
        })
    }

    /// Check that the current thread can make a mutable access without violating
    /// the Rust memory model.
    ///
    /// Specifically, this function checks that there is no concurrent mutable
    /// access and no concurrent immutable access(es) with this mutable access.
    pub fn check_mut(&self) {
        rt::execution(|execution| {
            trace!("CausalCell::check_mut");

            let thread_causality = &execution.threads.active().causality;
            let mut state = self.state.lock().unwrap();

            state.causality.check_mut(thread_causality).unwrap();
            state.causality.mut_access_version.join(thread_causality);

            for deferred in state.deferred.values_mut() {
                deferred.check_mut(thread_causality);
            }
        })
    }
}

impl CausalCheck {
    /// Panic if the CausaalCell access was invalid.
    pub fn check(mut self) {
        for (state, index) in self.deferred.drain(..) {
            let mut state = state.lock().unwrap();
            let deferred = state.deferred.remove(&index).unwrap();

            // panic if the check failed
            deferred.result.unwrap();

            if deferred.is_mut {
                state
                    .causality
                    .mut_access_version
                    .join(&deferred.thread_causality);
            } else {
                state
                    .causality
                    .immut_access_version
                    .join(&deferred.thread_causality);
            }

            // Validate all remaining deferred checks
            for other in state.deferred.values_mut() {
                if deferred.is_mut {
                    other.check_mut(&deferred.thread_causality);
                } else {
                    other.check(&deferred.thread_causality);
                }
            }
        }
    }

    /// Merge this check with another check
    pub fn join(&mut self, other: CausalCheck) {
        self.deferred.extend(other.deferred.into_iter());
    }
}

impl Default for CausalCheck {
    fn default() -> CausalCheck {
        CausalCheck { deferred: vec![] }
    }
}

impl Causality {
    fn check(&self, thread_causality: &VersionVec) -> Result<(), String> {
        // Check that there is no concurrent mutable access, i.e., the last
        // mutable access must happen-before this immutable access.

        // Negating the comparison as version vectors are not totally
        // ordered.
        if !(self.mut_access_version <= *thread_causality) {
            let msg = format!(
                "Causality violation: \
                 Concurrent mutable access and immutable access(es): \
                 cell.with: v={:?}; mut v: {:?}; thread={:?}",
                self.immut_access_version, self.mut_access_version, thread_causality
            );

            return Err(msg);
        }

        Ok(())
    }

    fn check_mut(&self, thread_causality: &VersionVec) -> Result<(), String> {
        // Check that there is no concurrent mutable access, i.e., the last
        // mutable access must happen-before this mutable access.

        // Negating the comparison as version vectors are not totally
        // ordered.
        if !(self.mut_access_version <= *thread_causality) {
            let msg = format!(
                "Causality violation: \
                 Concurrent mutable accesses: \
                 cell.with_mut: v={:?}; mut v={:?}; thread={:?}",
                self.immut_access_version, self.mut_access_version, thread_causality,
            );

            return Err(msg);
        }

        // Check that there are no concurrent immutable accesss, i.e., every
        // immutable access must happen-before this mutable access.
        //
        // Negating the comparison as version vectors are not totally
        // ordered.
        if !(self.immut_access_version <= *thread_causality) {
            let msg = format!(
                "Causality violation: \
                 Concurrent mutable access and immutable access(es): \
                 cell.with_mut: v={:?}; mut v={:?}; thread={:?}",
                self.immut_access_version, self.mut_access_version, thread_causality,
            );

            return Err(msg);
        }

        Ok(())
    }
}

impl Deferred {
    fn check(&mut self, thread_causality: &VersionVec) {
        if self.result.is_err() {
            return;
        }

        if !self.is_mut {
            // Concurrent reads are fine
            return;
        }

        // Mutable access w/ immutable access must not be concurrent
        if self
            .thread_causality
            .partial_cmp(thread_causality)
            .is_none()
        {
            self.result = Err(
                "Causality violation: concurrent mutable access and immutable access(es)"
                    .to_string(),
            );
        }
    }

    fn check_mut(&mut self, thread_causality: &VersionVec) {
        if self.result.is_err() {
            return;
        }

        // Mutable access w/ immutable access must not be concurrent
        if self
            .thread_causality
            .partial_cmp(thread_causality)
            .is_none()
        {
            self.result = Err("Causality violation: concurrent mutable accesses".to_string());
        }
    }
}
