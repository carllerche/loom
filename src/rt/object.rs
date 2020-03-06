use crate::rt::{alloc, arc, atomic, causal, condvar, execution, mutex, notify};
use crate::rt::{Access, Execution, VersionVec};
use bumpalo::{collections::vec::Vec as BumpVec, Bump};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct Object {
    /// Index in the store
    index: usize,

    /// Execution the object is part of
    execution_id: execution::Id,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct CausalCellRef {
    /// Index in the causal_cells vec
    index: usize,

    /// Execution the object is part of
    execution_id: execution::Id,
}

/// Stores objects
#[derive(Debug)]
pub struct Store<'bump> {
    /// Execution this store is part of
    execution_id: execution::Id,

    /// Stored state for all objects.
    entries: BumpVec<'bump, Entry<'bump>>,

    /// Stored state for CausalCells
    causal_cells: BumpVec<'bump, causal::State<'bump>>,

    /// Bump allocator
    bump: &'bump Bump,
}

impl Drop for Store<'_> {
    fn drop(&mut self) {
        // BumpVec won't drop its elements so we need to drop them manually because e.g.
        // condvar::State contains a VecDeque allocated on the heap so the drop is non-trivial.
        for object in self.entries.drain(..) {
            drop(object);
        }

        for object in self.causal_cells.drain(..) {
            drop(object);
        }
    }
}

/// Entry in the object store. Enumerates the different kinds of objects that
/// can be stored.
#[derive(Debug)]
enum Entry<'bump> {
    Alloc(alloc::State),
    Arc(arc::State<'bump>),
    Atomic(atomic::State<'bump>),
    Mutex(mutex::State<'bump>),
    Condvar(condvar::State<'bump>),
    Notify(notify::State<'bump>),
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
    pub(super) fn alloc<'a>(self, store: &'a mut Store<'_>) -> &'a mut alloc::State {
        assert_eq!(self.execution_id, store.execution_id);

        match &mut store.entries[self.index] {
            Entry::Alloc(v) => v,
            _ => panic!(),
        }
    }

    pub(super) fn arc_mut<'a, 'b>(self, store: &'a mut Store<'b>) -> &'a mut arc::State<'b> {
        assert_eq!(self.execution_id, store.execution_id);

        match &mut store.entries[self.index] {
            Entry::Arc(v) => v,
            _ => panic!(),
        }
    }

    pub(super) fn atomic_mut<'a, 'b>(
        self,
        store: &'a mut Store<'b>,
    ) -> Option<&'a mut atomic::State<'b>> {
        assert_eq!(self.execution_id, store.execution_id);

        match &mut store.entries[self.index] {
            Entry::Atomic(v) => Some(v),
            _ => None,
        }
    }

    pub(super) fn condvar_mut<'a, 'b>(
        self,
        store: &'a mut Store<'b>,
    ) -> Option<&'a mut condvar::State<'b>> {
        assert_eq!(self.execution_id, store.execution_id);

        match &mut store.entries[self.index] {
            Entry::Condvar(v) => Some(v),
            _ => None,
        }
    }

    pub(super) fn mutex_mut<'a, 'b>(
        self,
        store: &'a mut Store<'b>,
    ) -> Option<&'a mut mutex::State<'b>> {
        assert_eq!(self.execution_id, store.execution_id);

        match &mut store.entries[self.index] {
            Entry::Mutex(v) => Some(v),
            _ => None,
        }
    }

    pub(super) fn notify_mut<'a, 'b>(
        self,
        store: &'a mut Store<'b>,
    ) -> Option<&'a mut notify::State<'b>> {
        assert_eq!(self.execution_id, store.execution_id);

        match &mut store.entries[self.index] {
            Entry::Notify(v) => Some(v),
            _ => None,
        }
    }
}

impl CausalCellRef {
    pub(super) fn get_mut<'a, 'b>(self, store: &'a mut Store<'b>) -> &'a mut causal::State<'b> {
        assert_eq!(self.execution_id, store.execution_id);
        &mut store.causal_cells[self.index]
    }
}

impl<'bump> Store<'bump> {
    /// Create a new, empty, object store
    pub(super) fn new(execution_id: execution::Id, bump: &Bump) -> Store<'_> {
        Store {
            execution_id,
            entries: BumpVec::new_in(bump),
            causal_cells: BumpVec::new_in(bump),
            bump,
        }
    }

    /// Insert a new leak tracker
    pub(super) fn insert_alloc(&mut self, state: alloc::State) -> Object {
        self.insert(Entry::Alloc(state))
    }

    /// Insert a new arc object into the store.
    pub(super) fn insert_arc(&mut self, state: arc::State<'bump>) -> Object {
        self.insert(Entry::Arc(state))
    }

    /// Insert a new atomic object into the store.
    pub(super) fn insert_atomic(&mut self, state: atomic::State<'bump>) -> Object {
        self.insert(Entry::Atomic(state))
    }

    /// Iterate all atomic objects
    pub(super) fn atomics_mut(&mut self) -> impl Iterator<Item = &mut atomic::State<'bump>> {
        self.entries.iter_mut().filter_map(|entry| match entry {
            Entry::Atomic(entry) => Some(entry),
            _ => None,
        })
    }

    /// Insert a new mutex object into the store.
    pub(super) fn insert_mutex(&mut self, state: mutex::State<'bump>) -> Object {
        self.insert(Entry::Mutex(state))
    }

    /// Insert a new condition variable object into the store
    pub(super) fn insert_condvar(&mut self, state: condvar::State<'bump>) -> Object {
        self.insert(Entry::Condvar(state))
    }

    /// Inserts a new notify object into the store
    pub(super) fn insert_notify(&mut self, state: notify::State<'bump>) -> Object {
        self.insert(Entry::Notify(state))
    }

    fn insert(&mut self, entry: Entry<'bump>) -> Object {
        let index = self.entries.len();
        self.entries.push(entry);

        Object {
            index,
            execution_id: self.execution_id,
        }
    }

    pub(super) fn insert_causal_cell(&mut self, state: causal::State<'bump>) -> CausalCellRef {
        let index = self.causal_cells.len();
        self.causal_cells.push(state);

        CausalCellRef {
            index,
            execution_id: self.execution_id,
        }
    }

    pub(super) fn last_dependent_access(&self, operation: Operation) -> Option<&Access<'bump>> {
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
        dpor_vv: &VersionVec<'_>,
    ) {
        match &mut self.entries[operation.obj.index] {
            Entry::Alloc(_) => panic!("allocations are not branchable operations"),
            Entry::Arc(entry) => {
                entry.set_last_access(operation.action.into(), path_id, dpor_vv, self.bump)
            }
            Entry::Atomic(entry) => entry.set_last_access(path_id, dpor_vv, self.bump),
            Entry::Mutex(entry) => entry.set_last_access(path_id, dpor_vv, self.bump),
            Entry::Condvar(entry) => entry.set_last_access(path_id, dpor_vv, self.bump),
            Entry::Notify(entry) => entry.set_last_access(path_id, dpor_vv, self.bump),
        }
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
            self.set_action(execution, Action::Opaque);

            if is_locked {
                // The mutex is currently blocked, cannot make progress
                execution.threads.active_mut().set_blocked();
            }
        })
    }

    pub(super) fn branch<T: Into<Action>>(self, action: T) {
        super::branch(|execution| {
            self.set_action(execution, action.into());
        })
    }

    pub(super) fn branch_opaque(self) {
        self.branch(Action::Opaque)
    }

    fn set_action(self, execution: &mut Execution<'_>, action: Action) {
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
