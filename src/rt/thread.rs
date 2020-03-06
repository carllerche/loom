use crate::rt::execution;
use crate::rt::object::Operation;
use crate::rt::vv::VersionVec;

use bumpalo::Bump;
use std::{any::Any, collections::HashMap, fmt, ops};

pub(crate) struct Thread<'bump> {
    pub id: Id,

    /// If the thread is runnable, blocked, or terminated.
    pub state: State,

    /// True if the thread is in a critical section
    pub critical: bool,

    /// The operation the thread is about to take
    pub(super) operation: Option<Operation>,

    /// Tracks observed causality
    pub causality: VersionVec<'bump>,

    /// Tracks DPOR relations
    pub dpor_vv: VersionVec<'bump>,

    /// Version at which the thread last yielded
    pub last_yield: Option<usize>,

    /// Number of times the thread yielded
    pub yield_count: usize,

    locals: LocalMap,
}

#[derive(Debug)]
pub(crate) struct Set<'bump> {
    /// Unique execution identifier
    execution_id: execution::Id,

    /// Set of threads
    threads: Vec<Thread<'bump>>,

    /// Currently scheduled thread.
    ///
    /// `None` signifies that no thread is runnable.
    active: Option<usize>,

    /// Sequential consistency causality. All sequentially consistent operations
    /// synchronize with this causality.
    pub seq_cst_causality: VersionVec<'bump>,

    bump: &'bump Bump,
}

#[derive(Eq, PartialEq, Hash, Copy, Clone)]
pub(crate) struct Id {
    execution_id: execution::Id,
    id: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum State {
    Runnable,
    Blocked,
    Yield,
    Terminated,
}

type LocalMap = HashMap<LocalKeyId, LocalValue>;

#[derive(Eq, PartialEq, Hash, Copy, Clone)]
struct LocalKeyId(usize);

struct LocalValue(Option<Box<dyn Any>>);

impl<'bump> Thread<'bump> {
    fn new(id: Id, max_threads: usize, bump: &'bump Bump) -> Thread<'bump> {
        Thread {
            id,
            state: State::Runnable,
            critical: false,
            operation: None,
            causality: VersionVec::new_in(max_threads, bump),
            dpor_vv: VersionVec::new_in(max_threads, bump),
            last_yield: None,
            yield_count: 0,
            locals: HashMap::new(),
        }
    }

    pub(crate) fn is_runnable(&self) -> bool {
        match self.state {
            State::Runnable => true,
            _ => false,
        }
    }

    pub(crate) fn set_runnable(&mut self) {
        self.state = State::Runnable;
    }

    pub(crate) fn set_blocked(&mut self) {
        self.state = State::Blocked;
    }

    pub(crate) fn is_blocked(&self) -> bool {
        match self.state {
            State::Blocked => true,
            _ => false,
        }
    }

    pub(crate) fn is_yield(&self) -> bool {
        match self.state {
            State::Yield => true,
            _ => false,
        }
    }

    pub(crate) fn set_yield(&mut self) {
        self.state = State::Yield;
        self.last_yield = Some(self.causality[self.id]);
        self.yield_count += 1;
    }

    pub(crate) fn is_terminated(&self) -> bool {
        match self.state {
            State::Terminated => true,
            _ => false,
        }
    }

    pub(crate) fn set_terminated(&mut self) {
        self.state = State::Terminated;
    }

    pub(crate) fn drop_locals(&mut self) -> Box<dyn std::any::Any> {
        let mut locals = Vec::with_capacity(self.locals.len());

        // run the Drop impls of any mock thread-locals created by this thread.
        for (_, local) in &mut self.locals {
            locals.push(local.0.take());
        }

        Box::new(locals)
    }

    pub(crate) fn unpark(&mut self, unparker: &Thread<'_>) {
        self.causality.join(&unparker.causality);

        if self.is_blocked() || self.is_yield() {
            self.set_runnable();
        }
    }
}

impl fmt::Debug for Thread<'_> {
    // Manual debug impl is necessary because thread locals are represented as
    // `dyn Any`, which does not implement `Debug`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Thread")
            .field("id", &self.id)
            .field("state", &self.state)
            .field("critical", &self.critical)
            .field("operation", &self.operation)
            .field("causality", &self.causality)
            .field("dpor_vv", &self.dpor_vv)
            .field("last_yield", &self.last_yield)
            .field("yield_count", &self.yield_count)
            .field("locals", &format_args!("[..locals..]"))
            .finish()
    }
}

impl<'bump> Set<'bump> {
    /// Create an empty thread set.
    ///
    /// The set may contain up to `max_threads` threads.
    pub(crate) fn new(
        execution_id: execution::Id,
        max_threads: usize,
        bump: &'bump Bump,
    ) -> Set<'bump> {
        let mut threads = Vec::with_capacity(max_threads);

        // Push initial thread
        threads.push(Thread::new(Id::new(execution_id, 0), max_threads, bump));

        Set {
            execution_id,
            threads,
            active: Some(0),
            seq_cst_causality: VersionVec::new_in(max_threads, bump),
            bump,
        }
    }

    pub(crate) fn execution_id(&self) -> execution::Id {
        self.execution_id
    }

    /// Create a new thread
    pub(crate) fn new_thread(&mut self) -> Id {
        assert!(self.threads.len() < self.max());

        // Get the identifier for the thread about to be created
        let id = self.threads.len();
        let max_threads = self.threads.capacity();

        // Push the thread onto the stack
        self.threads.push(Thread::new(
            Id::new(self.execution_id, id),
            max_threads,
            self.bump,
        ));

        Id::new(self.execution_id, id)
    }

    pub(crate) fn max(&self) -> usize {
        self.threads.capacity()
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub(crate) fn active_id(&self) -> Id {
        Id::new(self.execution_id, self.active.unwrap())
    }

    pub(crate) fn active(&self) -> &Thread<'_> {
        &self.threads[self.active.unwrap()]
    }

    pub(crate) fn set_active(&mut self, id: Option<Id>) {
        self.active = id.map(Id::as_usize);
    }

    pub(crate) fn active_mut(&mut self) -> &mut Thread<'bump> {
        &mut self.threads[self.active.unwrap()]
    }

    /// Get the active thread and second thread
    pub(crate) fn active2_mut(&mut self, other: Id) -> (&mut Thread<'bump>, &mut Thread<'bump>) {
        let active = self.active.unwrap();
        let other = other.id;

        if other >= active {
            let (l, r) = self.threads.split_at_mut(other);

            (&mut l[active], &mut r[0])
        } else {
            let (l, r) = self.threads.split_at_mut(active);

            (&mut r[0], &mut l[other])
        }
    }

    pub(crate) fn active_causality_inc(&mut self) {
        let id = self.active_id();
        self.active_mut().causality.inc(id);
    }

    pub(crate) fn active_atomic_version(&self) -> usize {
        let id = self.active_id();
        self.active().causality[id]
    }

    pub(crate) fn unpark(&mut self, id: Id) {
        if id == self.active_id() {
            return;
        }

        // Synchronize memory
        let (active, th) = self.active2_mut(id);
        th.unpark(&active);
    }

    /// Insert a point of sequential consistency
    pub(crate) fn seq_cst(&mut self) {
        // The previous implementation of sequential consistency was incorrect.
        // As a quick fix, just disable it. This may fail to model correct code,
        // but will not silently allow bugs.
    }

    pub(crate) fn iter<'a>(&'a self) -> impl Iterator<Item = (Id, &'a Thread<'_>)> + 'a {
        let execution_id = self.execution_id;
        self.threads
            .iter()
            .enumerate()
            .map(move |(id, thread)| (Id::new(execution_id, id), thread))
    }

    pub(crate) fn iter_mut<'a>(&'a mut self) -> impl Iterator<Item = (Id, &'a mut Thread<'bump>)> {
        let execution_id = self.execution_id;
        self.threads
            .iter_mut()
            .enumerate()
            .map(move |(id, thread)| (Id::new(execution_id, id), thread))
    }

    /// Split the set of threads into the active thread and an iterator of all
    /// other threads.
    pub(crate) fn split_active(
        &mut self,
    ) -> (&mut Thread<'bump>, impl Iterator<Item = &mut Thread<'bump>>) {
        let active = self.active.unwrap();
        let (one, two) = self.threads.split_at_mut(active);
        let (active, two) = two.split_at_mut(1);

        let iter = one.iter_mut().chain(two.iter_mut());

        (&mut active[0], iter)
    }

    pub(crate) fn local<T: 'static>(
        &mut self,
        key: &'static crate::thread::LocalKey<T>,
    ) -> Option<Result<&T, AccessError>> {
        self.active_mut()
            .locals
            .get(&LocalKeyId::new(key))
            .map(|local_value| local_value.get())
    }

    pub(crate) fn local_init<T: 'static>(
        &mut self,
        key: &'static crate::thread::LocalKey<T>,
        value: T,
    ) {
        assert!(self
            .active_mut()
            .locals
            .insert(LocalKeyId::new(key), LocalValue::new(value))
            .is_none())
    }
}

impl<'bump> ops::Index<Id> for Set<'bump> {
    type Output = Thread<'bump>;

    fn index(&self, index: Id) -> &Thread<'bump> {
        &self.threads[index.id]
    }
}

impl<'bump> ops::IndexMut<Id> for Set<'bump> {
    fn index_mut(&mut self, index: Id) -> &mut Thread<'bump> {
        &mut self.threads[index.id]
    }
}

impl Id {
    pub(crate) fn new(execution_id: execution::Id, id: usize) -> Id {
        Id { execution_id, id }
    }

    pub(crate) fn as_usize(self) -> usize {
        self.id
    }
}

impl fmt::Display for Id {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.id.fmt(fmt)
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "Id({})", self.id)
    }
}

impl LocalKeyId {
    fn new<T>(key: &'static crate::thread::LocalKey<T>) -> Self {
        Self(key as *const _ as usize)
    }
}

impl LocalValue {
    fn new<T: 'static>(value: T) -> Self {
        Self(Some(Box::new(value)))
    }

    fn get<T: 'static>(&self) -> Result<&T, AccessError> {
        self.0
            .as_ref()
            .ok_or(AccessError { _private: () })
            .map(|val| {
                val.downcast_ref::<T>()
                    .expect("local value must downcast to expected type")
            })
    }
}

/// An error returned by [`LocalKey::try_with`](struct.LocalKey.html#method.try_with).
pub struct AccessError {
    _private: (),
}

impl fmt::Debug for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessError").finish()
    }
}

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("already destroyed", f)
    }
}
