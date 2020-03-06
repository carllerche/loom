use super::{critical, execution, object, VersionVec};

use std::collections::HashMap;

#[derive(Debug)]
pub(crate) struct CausalCell {
    obj_ref: object::CausalCellRef,
}

#[derive(Debug)]
pub(crate) struct State<'bump> {
    causality: Causality<'bump>,
    deferred: HashMap<usize, Deferred<'bump>>,
    next_index: usize,
}

#[derive(Debug)]
struct Causality<'bump> {
    // The transitive closure of all immutable accessses of `data`.
    immut_access_version: VersionVec<'bump>,

    // The last mutable access of `data`.
    mut_access_version: VersionVec<'bump>,
}

#[derive(Debug)]
pub struct CausalCheck {
    deferred: Vec<(object::CausalCellRef, usize)>,
}

#[derive(Debug)]
struct Deferred<'bump> {
    /// True if a mutable access
    is_mut: bool,

    /// Thread causality at the point the access happened.
    thread_causality: VersionVec<'bump>,

    /// Result
    result: Result<(), String>,
}

impl CausalCell {
    /// Construct a new instance of `CausalCell` which will wrap the specified
    /// value.
    pub(crate) fn new() -> CausalCell {
        execution(|execution| {
            let v = &execution.threads.active().causality;
            let obj_ref = execution.objects.insert_causal_cell(State {
                causality: Causality {
                    immut_access_version: v.clone_in(execution.bump),
                    mut_access_version: v.clone_in(execution.bump),
                },
                deferred: HashMap::new(),
                next_index: 0,
            });
            CausalCell { obj_ref }
        })
    }

    pub fn with<F, T, R>(&self, f: F, data: *const T) -> R
    where
        F: FnOnce(*const T) -> R,
    {
        critical(|| {
            self.check();
            f(data)
        })
    }

    pub(crate) fn with_deferred<F, T, R>(&self, f: F, data: *const T) -> (R, CausalCheck)
    where
        F: FnOnce(*const T) -> R,
    {
        critical(|| {
            execution(|execution| {
                let state = self.obj_ref.get_mut(&mut execution.objects);
                let thread_causality = &execution.threads.active().causality;

                let index = state.next_index;
                let result = state.causality.check(thread_causality);

                state.deferred.insert(
                    index,
                    Deferred {
                        is_mut: false,
                        thread_causality: thread_causality.clone_in(execution.bump),
                        result,
                    },
                );

                state.next_index += 1;

                let check = CausalCheck {
                    deferred: vec![(self.obj_ref.clone(), index)],
                };

                (f(data), check)
            })
        })
    }

    pub fn with_mut<F, T, R>(&self, f: F, data: *mut T) -> R
    where
        F: FnOnce(*mut T) -> R,
    {
        critical(|| {
            self.check_mut();
            f(data)
        })
    }

    pub fn with_deferred_mut<F, T, R>(&self, f: F, data: *mut T) -> (R, CausalCheck)
    where
        F: FnOnce(*mut T) -> R,
    {
        critical(|| {
            execution(|execution| {
                let state = self.obj_ref.get_mut(&mut execution.objects);
                let thread_causality = &execution.threads.active().causality;

                let index = state.next_index;
                let result = state.causality.check_mut(thread_causality);

                state.deferred.insert(
                    index,
                    Deferred {
                        is_mut: true,
                        thread_causality: thread_causality.clone_in(execution.bump),
                        result,
                    },
                );

                state.next_index += 1;

                let check = CausalCheck {
                    deferred: vec![(self.obj_ref.clone(), index)],
                };

                (f(data), check)
            })
        })
    }

    pub(crate) fn check(&self) {
        execution(|execution| {
            let state = self.obj_ref.get_mut(&mut execution.objects);
            let thread_causality = &execution.threads.active().causality;

            state.causality.check(thread_causality).unwrap();
            state.causality.immut_access_version.join(thread_causality);

            for deferred in state.deferred.values_mut() {
                deferred.check(thread_causality);
            }
        })
    }

    pub fn check_mut(&self) {
        execution(|execution| {
            let state = self.obj_ref.get_mut(&mut execution.objects);
            let thread_causality = &execution.threads.active().causality;

            state.causality.check_mut(thread_causality).unwrap();
            state.causality.mut_access_version.join(thread_causality);

            for deferred in state.deferred.values_mut() {
                deferred.check_mut(thread_causality);
            }
        })
    }
}

impl Causality<'_> {
    fn check(&self, thread_causality: &VersionVec<'_>) -> Result<(), String> {
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

    fn check_mut(&self, thread_causality: &VersionVec<'_>) -> Result<(), String> {
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

impl CausalCheck {
    /// Panic if the CausalCell access was invalid.
    pub fn check(mut self) {
        execution(|execution| {
            for (state_ref, index) in self.deferred.drain(..) {
                let state = state_ref.get_mut(&mut execution.objects);
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
        })
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

impl Deferred<'_> {
    fn check(&mut self, thread_causality: &VersionVec<'_>) {
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

    fn check_mut(&mut self, thread_causality: &VersionVec<'_>) {
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
