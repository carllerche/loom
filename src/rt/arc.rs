use crate::rt::object;
use crate::rt::{self, Access, Location, Synchronize, VersionVec};

use std::sync::atomic::Ordering::{Acquire, Release, SeqCst};

#[derive(Debug)]
pub(crate) struct Arc {
    state: object::Ref<State>,
}

#[derive(Debug)]
pub(super) struct State {
    /// Reference count
    ref_cnt: usize,

    /// Location where the arc was allocated
    allocated: Location,

    /// Causality transfers between threads
    ///
    /// Only updated on on ref dec and acquired before drop
    synchronize: Synchronize,

    /// Tracks access to the arc object
    last_ref_inc: Option<Access>,
    last_ref_dec: Option<Access>,
    last_ref_inspect: Option<Access>,
    last_ref_modification: Option<RefModify>,
}

/// Actions performed on the Arc
///
/// Clones are only dependent with inspections. Drops are dependent between each
/// other.
#[derive(Debug, Copy, Clone, PartialEq)]
pub(super) enum Action {
    /// Clone the arc
    RefInc,

    /// Drop the Arc
    RefDec,

    /// Inspect internals (such as get ref count). This is done with SeqCst
    /// causality
    Inspect,
}

/// Actions which modify the Arc's reference count
///
/// This is used to ascertain dependence for Action::Inspect
#[derive(Debug, Copy, Clone, PartialEq)]
enum RefModify {
    /// Corresponds to Action::RefInc
    RefInc,

    /// Corresponds to Action::RefDec
    RefDec,
}

impl Arc {
    pub(crate) fn new(location: Location) -> Arc {
        rt::execution(|execution| {
            let state = execution.objects.insert(State {
                ref_cnt: 1,
                allocated: location,
                synchronize: Synchronize::new(),
                last_ref_inc: None,
                last_ref_dec: None,
                last_ref_inspect: None,
                last_ref_modification: None,
            });

            Arc { state }
        })
    }

    pub(crate) fn ref_inc(&self) {
        self.branch(Action::RefInc);

        rt::execution(|execution| {
            let state = self.state.get_mut(&mut execution.objects);
            state.ref_cnt = state.ref_cnt.checked_add(1).expect("overflow");
        })
    }

    /// Validate a `get_mut` call
    pub(crate) fn get_mut(&self) -> bool {
        self.branch(Action::RefDec);

        rt::execution(|execution| {
            let state = self.state.get_mut(&mut execution.objects);

            assert!(state.ref_cnt >= 1, "Arc is released");

            // Synchronize the threads
            state.synchronize.sync_load(&mut execution.threads, Acquire);

            if state.ref_cnt == 1 {
                true
            } else {
                false
            }
        })
    }

    /// Returns true if the memory should be dropped.
    pub(crate) fn ref_dec(&self) -> bool {
        self.branch(Action::RefDec);

        rt::execution(|execution| {
            let state = self.state.get_mut(&mut execution.objects);

            assert!(state.ref_cnt >= 1, "Arc is already released");

            // Decrement the ref count
            state.ref_cnt -= 1;

            // Synchronize the threads.
            state
                .synchronize
                .sync_store(&mut execution.threads, Release);

            if state.ref_cnt == 0 {
                // Final ref count, the arc will be dropped. This requires
                // acquiring the causality
                //
                // In the real implementation, this is done with a fence.
                state.synchronize.sync_load(&mut execution.threads, Acquire);
                true
            } else {
                false
            }
        })
    }

    pub(crate) fn strong_count(&self) -> usize {
        self.branch(Action::Inspect);

        rt::execution(|execution| {
            let state = self.state.get_mut(&mut execution.objects);
            assert!(state.ref_cnt > 0, "Arc is already released");

            // Synchronize the threads.
            state.synchronize.sync_load(&mut execution.threads, SeqCst);

            state.ref_cnt
        })
    }

    fn branch(&self, action: Action) {
        let r = self.state;
        r.branch_action(action);
        assert!(
            r.ref_eq(self.state),
            "Internal state mutated during branch. This is \
                usually due to a bug in the algorithm being tested writing in \
                an invalid memory location."
        );
    }
}

impl State {
    pub(super) fn check_for_leaks(&self) {
        if self.ref_cnt != 0 {
            if self.allocated.is_captured() {
                panic!("Arc leaked.\n  Allocated: {}", self.allocated);
            } else {
                panic!("Arc leaked.");
            }
        }
    }

    pub(super) fn last_dependent_access(&self, action: Action) -> Option<&Access> {
        match action {
            // RefIncs are not dependent w/ RefDec, only inspections
            Action::RefInc => self.last_ref_inspect.as_ref(),
            Action::RefDec => self.last_ref_dec.as_ref(),
            Action::Inspect => match self.last_ref_modification {
                Some(RefModify::RefInc) => self.last_ref_inc.as_ref(),
                Some(RefModify::RefDec) => self.last_ref_dec.as_ref(),
                None => None,
            },
        }
    }

    pub(super) fn set_last_access(&mut self, action: Action, path_id: usize, version: &VersionVec) {
        match action {
            Action::RefInc => {
                self.last_ref_modification = Some(RefModify::RefInc);
                Access::set_or_create(&mut self.last_ref_inc, path_id, version)
            }
            Action::RefDec => {
                self.last_ref_modification = Some(RefModify::RefDec);
                Access::set_or_create(&mut self.last_ref_dec, path_id, version)
            }
            Action::Inspect => Access::set_or_create(&mut self.last_ref_inspect, path_id, version),
        }
    }
}
