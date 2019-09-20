use crate::rt::{self, thread, Access, Action, Path, Synchronize, VersionVec};

use std::sync::atomic::Ordering;

#[derive(Debug, Default)]
pub(crate) struct Atomic {
    last_load: Option<Access>,
    last_store: Option<Access>,
    history: History,
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
    pub(crate) fn initialize(&mut self, threads: &mut thread::Set) {
        self.history.stores.push(Store {
            sync: Synchronize::new(threads.max()),
            first_seen: FirstSeen::new(threads),
            seq_cst: false,
        });
    }

    pub(crate) fn load(
        &mut self,
        path: &mut Path,
        threads: &mut thread::Set,
        order: Ordering,
    ) -> usize {
        // Pick a store that satisfies causality and specified ordering.
        let index = self.history.pick_store(
            path,
            threads,
            order);

        self.history.stores[index].first_seen.touch(threads);
        self.history.stores[index].sync.sync_load(threads, order);
        index
    }

    pub(crate) fn store(&mut self, threads: &mut thread::Set, order: Ordering) {
        let mut store = Store {
            sync: Synchronize::new(threads.max()),
            first_seen: FirstSeen::new(threads),
            seq_cst: is_seq_cst(order),
        };

        store.sync.sync_store(threads, order);
        self.history.stores.push(store);
    }

    pub(crate) fn rmw<F, E>(
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

    pub(crate) fn happens_before(&self, vv: &VersionVec) {
        assert!({
            self.history.stores
                .iter()
                .all(|store| vv >= store.sync.version_vec())
        });
    }

    pub(crate) fn last_dependent_accesses<'a>(
        &'a self,
        action: Action,
    ) -> Box<dyn Iterator<Item = &'a Access> + 'a> {
        match action {
            Action::Load => Box::new(self.last_store.iter()),
            Action::Store => Box::new(self.last_load.iter()),
            Action::Rmw => Box::new({ self.last_load.iter().chain(self.last_store.iter()) }),
            _ => unreachable!(),
        }
    }

    pub(crate) fn set_last_access(&mut self, action: Action, access: Access) {
        match action {
            Action::Load => self.last_load = Some(access),
            Action::Store => self.last_store = Some(access),
            Action::Rmw => {
                self.last_load = Some(access.clone());
                self.last_store = Some(access);
            }
            _ => unreachable!(),
        }
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
                    in_causality |= store.first_seen.is_seen_by(&threads);

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

    fn is_seen_by(&self, threads: &thread::Set) -> bool {
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
