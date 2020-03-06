use crate::rt::object::{self, Object};
use crate::rt::{self, thread, Access, Mutex, VersionVec};

use bumpalo::Bump;
use std::collections::VecDeque;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) struct Condvar {
    obj: Object,
}

#[derive(Debug)]
pub(super) struct State<'bump> {
    /// Tracks access to the mutex
    last_access: Option<Access<'bump>>,

    /// Threads waiting on the condvar
    pub(crate) waiters: VecDeque<thread::Id>,
}

impl Condvar {
    /// Create a new condition variable object
    pub(crate) fn new() -> Condvar {
        super::execution(|execution| {
            let obj = execution.objects.insert_condvar(State {
                last_access: None,
                waiters: VecDeque::new(),
            });

            Condvar { obj }
        })
    }

    /// Blocks the current thread until this condition variable receives a notification.
    pub(crate) fn wait(&self, mutex: &Mutex) {
        self.obj.branch_opaque();

        rt::execution(|execution| {
            let state = self.get_state(&mut execution.objects);

            // Track the current thread as a waiter
            state.waiters.push_back(execution.threads.active_id());
        });

        // Release the lock
        mutex.release_lock();

        // Disable the current thread
        rt::park();

        // Acquire the lock again
        mutex.acquire_lock();
    }

    /// Wakes up one blocked thread on this condvar.
    pub(crate) fn notify_one(&self) {
        self.obj.branch_opaque();

        rt::execution(|execution| {
            let state = self.get_state(&mut execution.objects);

            // Notify the first waiter
            let thread = state.waiters.pop_front();

            if let Some(thread) = thread {
                execution.threads.unpark(thread);
            }
        })
    }

    /// Wakes up all blocked threads on this condvar.
    pub(crate) fn notify_all(&self) {
        self.obj.branch_opaque();

        rt::execution(|execution| {
            let state = self.get_state(&mut execution.objects);

            for thread in state.waiters.drain(..) {
                execution.threads.unpark(thread);
            }
        })
    }

    fn get_state<'a, 'bump>(&self, store: &'a mut object::Store<'bump>) -> &'a mut State<'bump> {
        self.obj.condvar_mut(store).unwrap()
    }
}

impl<'bump> State<'bump> {
    pub(super) fn last_dependent_access(&self) -> Option<&Access<'bump>> {
        self.last_access.as_ref()
    }

    pub(crate) fn set_last_access(
        &mut self,
        path_id: usize,
        version: &VersionVec<'_>,
        bump: &'bump Bump,
    ) {
        Access::set_or_create_in(&mut self.last_access, path_id, version, bump);
    }
}
