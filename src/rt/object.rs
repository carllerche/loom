use crate::rt;
use crate::rt::{Access, Execution, VersionVec};

use std::fmt;
use std::marker::PhantomData;

#[cfg(feature = "checkpoint")]
use serde::{Deserialize, Serialize};

/// Stores objects
#[derive(Debug)]
#[cfg_attr(feature = "checkpoint", derive(Serialize, Deserialize))]
pub(super) struct Store<T = Entry> {
    /// Stored state for all objects.
    entries: Vec<T>,
}

pub(super) trait Object: Sized {
    type Entry;

    /// Convert an object into an entry
    fn into_entry(self) -> Self::Entry;

    /// Convert an entry ref into an object ref
    fn get_ref(entry: &Self::Entry) -> Option<&Self>;

    /// Convert a mutable entry ref into a mutable object ref
    fn get_mut(entry: &mut Self::Entry) -> Option<&mut Self>;
}

/// References an object in the store.
///
/// The reference tracks the type it references. Using `()` indicates the type
/// is unknown.
#[derive(Eq, PartialEq)]
#[cfg_attr(feature = "checkpoint", derive(Serialize, Deserialize))]
pub(super) struct Ref<T = ()> {
    /// Index in the store
    index: usize,

    _p: PhantomData<T>,
}

// TODO: mov to separate file
#[derive(Debug, Copy, Clone)]
pub(super) struct Operation {
    obj: Ref,
    action: Action,
}

// TODO: move to separate file
#[derive(Debug, Copy, Clone)]
pub(super) enum Action {
    /// Action on an Arc object
    Arc(rt::arc::Action),

    /// Action on an atomic object
    Atomic(rt::atomic::Action),

    /// Generic action with no specialized dependencies on access.
    Opaque,
}

macro_rules! objects {
    ( $(#[$attrs:meta])* $e:ident, $( $name:ident($ty:path), )* ) => {

        $(#[$attrs])*
        pub(super) enum $e {

            $(
                $name($ty),
            )*
        }

        $(
            impl crate::rt::object::Object for $ty {
                type Entry = $e;

                fn into_entry(self) -> Entry {
                    $e::$name(self)
                }

                fn get_ref(entry: &Entry) -> Option<&$ty> {
                    match entry {
                        $e::$name(obj) => Some(obj),
                        _ => None,
                    }
                }

                fn get_mut(entry: &mut Entry) -> Option<&mut $ty> {
                    match entry {
                        $e::$name(obj) => Some(obj),
                        _ => None,
                    }
                }
            }
        )*
    };
}

objects! {
    #[derive(Debug)]
    Entry,

    // State tracking allocations. Used for leak detection.
    Alloc(rt::alloc::State),

    // State associated with a modeled `Arc`.
    Arc(rt::arc::State),

    // State associated with an atomic cell
    Atomic(rt::atomic::State),

    // State associated with a mutex.
    Mutex(rt::mutex::State),

    // State associated with a modeled condvar.
    Condvar(rt::condvar::State),

    // State associated with a modeled thread notifier.
    Notify(rt::notify::State),

    // Tracks access to a memory cell
    Cell(rt::cell::State),
}

impl<T> Store<T> {
    /// Create a new, empty, object store
    pub(super) fn with_capacity(capacity: usize) -> Store<T> {
        Store {
            entries: Vec::with_capacity(capacity),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn capacity(&self) -> usize {
        self.entries.capacity()
    }

    pub(super) fn reserve_exact(&mut self, additional: usize) {
        self.entries.reserve_exact(additional);
    }

    /// Insert an object into the store
    pub(super) fn insert<O>(&mut self, item: O) -> Ref<O>
    where
        O: Object<Entry = T>,
    {
        let index = self.entries.len();
        self.entries.push(item.into_entry());

        Ref {
            index,
            _p: PhantomData,
        }
    }

    pub(crate) fn truncate<O>(&mut self, obj: Ref<O>) {
        let target = obj.index + 1;
        self.entries.truncate(target);
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    pub(super) fn iter_ref<'a, O>(&'a self) -> impl DoubleEndedIterator<Item = Ref<O>> + 'a
    where
        O: Object<Entry = T>,
    {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| O::get_ref(e).is_some())
            .map(|(index, _)| Ref {
                index,
                _p: PhantomData,
            })
    }

    pub(super) fn iter_mut<'a, O>(&'a mut self) -> impl DoubleEndedIterator<Item = &mut O> 
    where
        O: Object<Entry = T> + 'a,
    {
        self.entries.iter_mut().filter_map(O::get_mut)
    }
}

impl Store {
    pub(super) fn last_dependent_access(&self, operation: Operation) -> Option<&Access> {
        match &self.entries[operation.obj.index] {
            Entry::Arc(entry) => entry.last_dependent_access(operation.action.into()),
            Entry::Atomic(entry) => entry.last_dependent_access(operation.action.into()),
            Entry::Mutex(entry) => entry.last_dependent_access(),
            Entry::Condvar(entry) => entry.last_dependent_access(),
            Entry::Notify(entry) => entry.last_dependent_access(),
            obj => panic!(
                "object is not branchable {:?}; ref = {:?}",
                obj, operation.obj
            ),
        }
    }

    pub(super) fn set_last_access(
        &mut self,
        operation: Operation,
        path_id: usize,
        dpor_vv: &VersionVec,
    ) {
        match &mut self.entries[operation.obj.index] {
            Entry::Arc(entry) => entry.set_last_access(operation.action.into(), path_id, dpor_vv),
            Entry::Atomic(entry) => {
                entry.set_last_access(operation.action.into(), path_id, dpor_vv)
            }
            Entry::Mutex(entry) => entry.set_last_access(path_id, dpor_vv),
            Entry::Condvar(entry) => entry.set_last_access(path_id, dpor_vv),
            Entry::Notify(entry) => entry.set_last_access(path_id, dpor_vv),
            _ => panic!("object is not branchable"),
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

impl<T> Ref<T> {
    /// Erase the type marker
    pub(super) fn erase(self) -> Ref<()> {
        Ref {
            index: self.index,
            _p: PhantomData,
        }
    }

    pub(super) fn ref_eq(self, other: Ref<T>) -> bool {
        self.index == other.index
    }
}

impl<T: Object> Ref<T> {
    /// Get a reference to the object associated with this reference from the store
    pub(super) fn get(self, store: &Store<T::Entry>) -> &T {
        T::get_ref(&store.entries[self.index]).expect("unexpected object stored at reference")
    }

    /// Get a mutable reference to the object associated with this reference
    /// from the store
    pub(super) fn get_mut(self, store: &mut Store<T::Entry>) -> &mut T {
        T::get_mut(&mut store.entries[self.index]).expect("unexpected object stored at reference")
    }
}

impl Ref {
    /// Convert a store index `usize` into a ref
    pub(super) fn from_usize(index: usize) -> Ref {
        Ref {
            index,
            _p: PhantomData,
        }
    }

    pub(super) fn downcast<T>(self, store: &Store<T::Entry>) -> Option<Ref<T>>
    where
        T: Object,
    {
        T::get_ref(&store.entries[self.index]).map(|_| Ref {
            index: self.index,
            _p: PhantomData,
        })
    }
}

impl<T> Clone for Ref<T> {
    fn clone(&self) -> Ref<T> {
        Ref {
            index: self.index,
            _p: PhantomData,
        }
    }
}

impl<T> Copy for Ref<T> {}

impl<T> fmt::Debug for Ref<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        use std::any::type_name;

        write!(fmt, "Ref<{}>({})", type_name::<T>(), self.index)
    }
}

// TODO: These fns shouldn't be on Ref
impl<T: Object<Entry = Entry>> Ref<T> {
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

    pub(super) fn branch_action(self, action: impl Into<Action>) {
        super::branch(|execution| {
            self.set_action(execution, action.into());
        })
    }

    pub(super) fn branch_opaque(self) {
        self.branch_action(Action::Opaque)
    }

    fn set_action(self, execution: &mut Execution, action: Action) {
        assert!(
            T::get_ref(&execution.objects.entries[self.index]).is_some(),
            "failed to get object for ref {:?}",
            self
        );

        execution.threads.active_mut().operation = Some(Operation {
            obj: self.erase(),
            action,
        });
    }
}

impl Operation {
    pub(super) fn object(&self) -> Ref {
        self.obj
    }
}

impl Into<rt::arc::Action> for Action {
    fn into(self) -> rt::arc::Action {
        match self {
            Action::Arc(action) => action,
            _ => unreachable!(),
        }
    }
}

impl Into<rt::atomic::Action> for Action {
    fn into(self) -> rt::atomic::Action {
        match self {
            Action::Atomic(action) => action,
            _ => unreachable!(),
        }
    }
}

impl Into<Action> for rt::arc::Action {
    fn into(self) -> Action {
        Action::Arc(self)
    }
}

impl Into<Action> for rt::atomic::Action {
    fn into(self) -> Action {
        Action::Atomic(self)
    }
}
