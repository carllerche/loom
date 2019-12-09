use crate::rt::object::Object;
use crate::rt::{self, thread, Access, Path, Synchronize, VersionVec};

use std::sync::atomic::Ordering;
use std::sync::atomic::Ordering::Acquire;

use tracing::trace;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) struct Atomic {
    obj: Object,
}

#[derive(Debug, Default)]
pub(super) struct State {
    last_access: Option<Access>,
    history: History,
}

#[derive(Debug, Copy, Clone)]
pub(super) enum Action {
    /// Atomic load
    Load,

    /// Atomic store
    Store,

    /// Atomic read-modify-write
    Rmw,
}

#[derive(Debug, Default)]
struct History {
    stores: Vec<Store>,
}

#[derive(Debug)]
struct Store {
    /// Manages causality transfers between threads
    sync: Synchronize,

    /// Tracks when each thread first saw value
    first_seen: FirstSeen,

    /// True when the store was done with `SeqCst` ordering
    seq_cst: bool,
}

#[derive(Debug)]
struct FirstSeen(Vec<Option<usize>>);

impl Atomic {
    pub(crate) fn new() -> Atomic {
        rt::execution(|execution| {
            let mut state = State::default();

            // All atomics are initialized with a value, which brings the causality
            // of the thread initializing the atomic.
            state.history.stores.push(Store {
                sync: Synchronize::new(execution.threads.max()),
                first_seen: FirstSeen::new(&mut execution.threads),
                seq_cst: false,
            });

            let obj = execution.objects.insert_atomic(state);

            trace!(?obj, "Atomic::new");

            Atomic { obj }
        })
    }

    pub(crate) fn load(self, order: Ordering) -> usize {
        self.obj.branch(Action::Load);

        super::synchronize(|execution| {
            trace!(obj = ?self.obj, ?order, "Atomic::load");

            self.obj.atomic_mut(&mut execution.objects).unwrap().load(
                &mut execution.path,
                &mut execution.threads,
                order,
            )
        })
    }

    pub(crate) fn store(self, order: Ordering) {
        self.obj.branch(Action::Store);

        super::synchronize(|execution| {
            trace!(obj = ?self.obj, ?order, "Atomic::store");

            self.obj
                .atomic_mut(&mut execution.objects)
                .unwrap()
                .store(&mut execution.threads, order)
        })
    }

    pub(crate) fn rmw<F, E>(self, f: F, success: Ordering, failure: Ordering) -> Result<usize, E>
    where
        F: FnOnce(usize) -> Result<(), E>,
    {
        self.obj.branch(Action::Rmw);

        super::synchronize(|execution| {
            trace!(obj = ?self.obj, ?success, ?failure, "Atomic::rmw");

            self.obj.atomic_mut(&mut execution.objects).unwrap().rmw(
                f,
                &mut execution.threads,
                success,
                failure,
            )
        })
    }

    /// Assert that the entire atomic history happens before the current thread.
    /// This is required to safely call `get_mut()`.
    pub(crate) fn get_mut(self) {
        // TODO: Is this needed?
        self.obj.branch(Action::Rmw);

        super::execution(|execution| {
            trace!(obj = ?self.obj, "Atomic::get_mut");

            self.obj
                .atomic_mut(&mut execution.objects)
                .unwrap()
                .happens_before(&execution.threads.active().causality);
        });
    }
}

pub(crate) fn fence(order: Ordering) {
    assert_eq!(
        order, Acquire,
        "only Acquire fences are currently supported"
    );

    rt::synchronize(|execution| {
        trace!(?order, "fence");

        // Find all stores for all atomic objects and, if they have been read by
        // the current thread, establish an acquire synchronization.
        for state in execution.objects.atomics_mut() {
            // Iterate all the stores
            for store in &mut state.history.stores {
                if !store.first_seen.is_seen_by_current(&execution.threads) {
                    continue;
                }

                store.sync.sync_load(&mut execution.threads, order);
            }
        }
    });
}

impl State {
    pub(super) fn last_dependent_access(&self) -> Option<&Access> {
        self.last_access.as_ref()
    }

    pub(super) fn set_last_access(&mut self, path_id: usize, version: &VersionVec) {
        Access::set_or_create(&mut self.last_access, path_id, version);
    }

    fn load(&mut self, path: &mut Path, threads: &mut thread::Set, order: Ordering) -> usize {
        // Pick a store that satisfies causality and specified ordering.
        let index = self.history.pick_store(path, threads, order);

        self.history.stores[index].first_seen.touch(threads);
        self.history.stores[index].sync.sync_load(threads, order);
        index
    }

    fn store(&mut self, threads: &mut thread::Set, order: Ordering) {
        let mut store = Store {
            sync: Synchronize::new(threads.max()),
            first_seen: FirstSeen::new(threads),
            seq_cst: is_seq_cst(order),
        };

        store.sync.sync_store(threads, order);
        self.history.stores.push(store);
    }

    fn rmw<F, E>(
        &mut self,
        f: F,
        threads: &mut thread::Set,
        success: Ordering,
        failure: Ordering,
    ) -> Result<usize, E>
    where
        F: FnOnce(usize) -> Result<(), E>,
    {
        let index = self.history.stores.len() - 1;
        self.history.stores[index].first_seen.touch(threads);

        if let Err(e) = f(index) {
            self.history.stores[index].sync.sync_load(threads, failure);
            return Err(e);
        }

        self.history.stores[index].sync.sync_load(threads, success);

        let mut new = Store {
            // Clone the previous sync in order to form a release sequence.
            sync: self.history.stores[index].sync.clone(),
            first_seen: FirstSeen::new(threads),
            seq_cst: is_seq_cst(success),
        };

        new.sync.sync_store(threads, success);
        self.history.stores.push(new);

        Ok(index)
    }

    fn happens_before(&self, vv: &VersionVec) {
        assert!({
            self.history
                .stores
                .iter()
                .all(|store| vv >= store.sync.version_vec())
        });
    }
}

impl History {
    fn pick_store(
        &mut self,
        path: &mut rt::Path,
        threads: &mut thread::Set,
        order: Ordering,
    ) -> usize {
        let mut in_causality = false;
        let mut first = true;

        path.branch_write({
            self.stores
                .iter()
                .enumerate()
                .rev()
                // Explore all writes that are not within the actor's causality as
                // well as the latest one.
                .take_while(|&(_, ref store)| {
                    let ret = in_causality;

                    if store.first_seen.is_seen_before_yield(&threads) {
                        let ret = first;
                        in_causality = true;
                        first = false;
                        return ret;
                    }

                    first = false;

                    in_causality |= is_seq_cst(order) && store.seq_cst;
                    in_causality |= store.first_seen.is_seen_by_current(&threads);

                    !ret
                })
                .map(|(i, _)| i)
        })
    }
}

impl FirstSeen {
    fn new(threads: &mut thread::Set) -> FirstSeen {
        let mut first_seen = FirstSeen(vec![]);
        first_seen.touch(threads);

        first_seen
    }

    fn touch(&mut self, threads: &thread::Set) {
        let happens_before = &threads.active().causality;

        if self.0.len() < happens_before.len() {
            self.0.resize(happens_before.len(), None);
        }

        if self.0[threads.active_id().as_usize()].is_none() {
            self.0[threads.active_id().as_usize()] = Some(threads.active_atomic_version());
        }
    }

    fn is_seen_by_current(&self, threads: &thread::Set) -> bool {
        for (thread_id, version) in threads.active().causality.versions(threads.execution_id()) {
            let seen = self
                .0
                .get(thread_id.as_usize())
                .and_then(|maybe_version| *maybe_version)
                .map(|v| v <= version)
                .unwrap_or(false);

            if seen {
                return true;
            }
        }

        false
    }

    fn is_seen_before_yield(&self, threads: &thread::Set) -> bool {
        let thread_id = threads.active_id();

        let last_yield = match threads.active().last_yield {
            Some(v) => v,
            None => return false,
        };

        match self.0[thread_id.as_usize()] {
            None => false,
            Some(v) => v <= last_yield,
        }
    }
}

fn is_seq_cst(order: Ordering) -> bool {
    match order {
        Ordering::SeqCst => true,
        _ => false,
    }
}
