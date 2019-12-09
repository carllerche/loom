use crate::rt::{alloc, arc, atomic, condvar, execution, mutex, notify};
use crate::rt::{Access, Execution, VersionVec};

use tracing::trace;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct Object {
    /// Index in the store
    index: usize,

    /// Execution the object is part of
    execution_id: execution::Id,
}

/// Stores objects
#[derive(Debug)]
pub struct Store {
    /// Execution this store is part of
    execution_id: execution::Id,

    /// Stored state for all objects.
    entries: Vec<Entry>,
}

/// Entry in the object store. Enumerates the different kinds of objects that
/// can be stored.
#[derive(Debug)]
enum Entry {
    Alloc(alloc::State),
    Arc(arc::State),
    Atomic(atomic::State),
    Mutex(mutex::State),
    Condvar(condvar::State),
    Notify(notify::State),
}

// TODO: mov to separate file
#[derive(Debug, Copy, Clone)]
pub(super) struct Operation {
    obj: Object,
    action: Action,
}

// TODO: move to separate file
#[derive(Debug, Copy, Clone)]
pub(super) enum Action {
    /// Action on an Arc object
    Arc(arc::Action),

    /// Action on an atomic object
    Atomic(atomic::Action),

    /// Generic action with no specialized dependencies on access.
    Opaque,
}

impl Object {
    pub(super) fn alloc(self, store: &mut Store) -> &mut alloc::State {
        assert_eq!(self.execution_id, store.execution_id);

        match &mut store.entries[self.index] {
            Entry::Alloc(v) => v,
            _ => panic!(),
        }
    }

    pub(super) fn arc_mut(self, store: &mut Store) -> &mut arc::State {
        assert_eq!(self.execution_id, store.execution_id);

        match &mut store.entries[self.index] {
            Entry::Arc(v) => v,
            _ => panic!(),
        }
    }

    pub(super) fn atomic_mut(self, store: &mut Store) -> Option<&mut atomic::State> {
        assert_eq!(self.execution_id, store.execution_id);

        match &mut store.entries[self.index] {
            Entry::Atomic(v) => Some(v),
            _ => None,
        }
    }

    pub(super) fn condvar_mut(self, store: &mut Store) -> Option<&mut condvar::State> {
        assert_eq!(self.execution_id, store.execution_id);

        match &mut store.entries[self.index] {
            Entry::Condvar(v) => Some(v),
            _ => None,
        }
    }

    pub(super) fn mutex_mut(self, store: &mut Store) -> Option<&mut mutex::State> {
        assert_eq!(self.execution_id, store.execution_id);

        match &mut store.entries[self.index] {
            Entry::Mutex(v) => Some(v),
            _ => None,
        }
    }

    pub(super) fn notify_mut(self, store: &mut Store) -> Option<&mut notify::State> {
        assert_eq!(self.execution_id, store.execution_id);

        match &mut store.entries[self.index] {
            Entry::Notify(v) => Some(v),
            _ => None,
        }
    }
}

impl Store {
    /// Create a new, empty, object store
    pub(super) fn new(execution_id: execution::Id) -> Store {
        Store {
            execution_id,
            entries: vec![],
        }
    }

    /// Insert a new leak tracker
    pub(super) fn insert_alloc(&mut self, state: alloc::State) -> Object {
        self.insert(Entry::Alloc(state))
    }

    /// Insert a new arc object into the store.
    pub(super) fn insert_arc(&mut self, state: arc::State) -> Object {
        self.insert(Entry::Arc(state))
    }

    /// Insert a new atomic object into the store.
    pub(super) fn insert_atomic(&mut self, state: atomic::State) -> Object {
        self.insert(Entry::Atomic(state))
    }

    /// Iterate all atomic objects
    pub(super) fn atomics_mut(&mut self) -> impl Iterator<Item = &mut atomic::State> {
        self.entries.iter_mut().filter_map(|entry| match entry {
            Entry::Atomic(entry) => Some(entry),
            _ => None,
        })
    }

    /// Insert a new mutex object into the store.
    pub(super) fn insert_mutex(&mut self, state: mutex::State) -> Object {
        self.insert(Entry::Mutex(state))
    }

    /// Insert a new condition variable object into the store
    pub(super) fn insert_condvar(&mut self, state: condvar::State) -> Object {
        self.insert(Entry::Condvar(state))
    }

    /// Inserts a new notify object into the store
    pub(super) fn insert_notify(&mut self, state: notify::State) -> Object {
        self.insert(Entry::Notify(state))
    }

    fn insert(&mut self, entry: Entry) -> Object {
        let index = self.entries.len();
        self.entries.push(entry);

        Object {
            index,
            execution_id: self.execution_id,
        }
    }

    pub(super) fn last_dependent_access(&self, operation: Operation) -> Option<&Access> {
        match &self.entries[operation.obj.index] {
            Entry::Alloc(_) => panic!("allocations are not branchable operations"),
            Entry::Arc(entry) => entry.last_dependent_access(operation.action.into()),
            Entry::Atomic(entry) => entry.last_dependent_access(),
            Entry::Mutex(entry) => entry.last_dependent_access(),
            Entry::Condvar(entry) => entry.last_dependent_access(),
            Entry::Notify(entry) => entry.last_dependent_access(),
        }
    }

    pub(super) fn set_last_access(
        &mut self,
        operation: Operation,
        path_id: usize,
        dpor_vv: &VersionVec,
    ) {
        match &mut self.entries[operation.obj.index] {
            Entry::Alloc(_) => panic!("allocations are not branchable operations"),
            Entry::Arc(entry) => entry.set_last_access(operation.action.into(), path_id, dpor_vv),
            Entry::Atomic(entry) => entry.set_last_access(path_id, dpor_vv),
            Entry::Mutex(entry) => entry.set_last_access(path_id, dpor_vv),
            Entry::Condvar(entry) => entry.set_last_access(path_id, dpor_vv),
            Entry::Notify(entry) => entry.set_last_access(path_id, dpor_vv),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    /// Panics if any leaks were detected
    pub(crate) fn check_for_leaks(&self) {
        for entry in &self.entries[..] {
            match entry {
                Entry::Alloc(entry) => entry.check_for_leaks(),
                Entry::Arc(entry) => entry.check_for_leaks(),
                _ => {}
            }
        }
    }
}

impl Object {
    // TODO: rename `branch_disable`
    pub(super) fn branch_acquire(self, is_locked: bool) {
        super::branch(|execution| {
            trace!(obj = ?self, ?is_locked, "Object::branch_acquire");

            self.set_action(execution, Action::Opaque);

            if is_locked {
                // The mutex is currently blocked, cannot make progress
                execution.threads.active_mut().set_blocked();
            }
        })
    }

    pub(super) fn branch<T: Into<Action>>(self, t: T) {
        super::branch(|execution| {
            let action = t.into();

            trace!(obj = ?self, ?action, "Object::branch");

            self.set_action(execution, action);
        })
    }

    pub(super) fn branch_opaque(self) {
        self.branch(Action::Opaque)
    }

    fn set_action(self, execution: &mut Execution, action: Action) {
        execution.threads.active_mut().operation = Some(Operation { obj: self, action });
    }
}

impl Operation {
    pub(super) fn object(&self) -> Object {
        self.obj
    }
}

impl Into<arc::Action> for Action {
    fn into(self) -> arc::Action {
        match self {
            Action::Arc(action) => action,
            _ => unimplemented!(),
        }
    }
}

impl Into<atomic::Action> for Action {
    fn into(self) -> atomic::Action {
        match self {
            Action::Atomic(action) => action,
            _ => unimplemented!(),
        }
    }
}

impl Into<Action> for arc::Action {
    fn into(self) -> Action {
        Action::Arc(self)
    }
}

impl Into<Action> for atomic::Action {
    fn into(self) -> Action {
        Action::Atomic(self)
    }
}
