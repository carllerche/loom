#![allow(warnings)]
use crate::rt::object::Object;
use crate::rt::{self, Access, Synchronize, VersionVec};

use std::sync::atomic::Ordering::{Acquire, Release};

use tracing::trace;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Arc {
    obj: Object,
}

#[derive(Debug)]
pub(super) struct State {
    /// Reference count
    ref_cnt: usize,

    /// Causality transfers between threads
    ///
    /// Only updated on on ref dec and acquired before drop
    synchronize: Synchronize,

    /// Tracks access to the arc object
    last_ref_inc: Option<Access>,
    last_ref_dec: Option<Access>,
}

/// Actions performed on the Arc
///
/// Clones are only dependent with inspections. Drops are dependent between each
/// other.
#[derive(Debug, Copy, Clone)]
pub(super) enum Action {
    /// Clone the arc
    RefInc,

    /// Drop the Arc
    RefDec,
    /*
    /// Inspect internals (such as get ref count). This is done with SeqCst
    /// causality
    Inspect,
    */
}

impl Arc {
    pub(crate) fn new() -> Arc {
        rt::execution(|execution| {
            let obj = execution.objects.insert_arc(State {
                ref_cnt: 1,
                synchronize: Synchronize::new(execution.max_threads),
                last_ref_inc: None,
                last_ref_dec: None,
            });

            trace!(?obj, "Arc::new");

            Arc { obj }
        })
    }

    pub(crate) fn ref_inc(self) {
        self.obj.branch(Action::RefInc);

        rt::execution(|execution| {
            let state = self.obj.arc_mut(&mut execution.objects);
            state.ref_cnt = state.ref_cnt.checked_add(1).expect("overflow");

            trace!(obj = ?self.obj, ref_cnt = ?state.ref_cnt, "Arc::ref_inc");
        })
    }

    /// Validate a `get_mut` call
    pub(crate) fn get_mut(self) -> bool {
        self.obj.branch(Action::RefDec);

        rt::execution(|execution| {
            let state = self.obj.arc_mut(&mut execution.objects);

            assert!(state.ref_cnt >= 1, "Arc is released");

            // Synchronize the threads
            state.synchronize.sync_load(&mut execution.threads, Acquire);

            let is_only_ref = state.ref_cnt == 1;

            trace!(obj = ?self.obj, ?is_only_ref, "Arc::get_mut");

            is_only_ref
        })
    }

    /// Returns true if the memory should be dropped.
    pub(crate) fn ref_dec(self) -> bool {
        self.obj.branch(Action::RefDec);

        rt::execution(|execution| {
            let state = self.obj.arc_mut(&mut execution.objects);

            assert!(state.ref_cnt >= 1, "Arc is already released");

            // Decrement the ref count
            state.ref_cnt -= 1;

            trace!(obj = ?self.obj, ref_cnt = ?state.ref_cnt, "Arc::ref_dec");

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
}

impl State {
    pub(super) fn check_for_leaks(&self) {
        assert_eq!(0, self.ref_cnt, "Arc leaked");
    }

    pub(super) fn last_dependent_access(&self, action: Action) -> Option<&Access> {
        match action {
            // RefIncs are not dependent w/ RefDec, only inspections
            Action::RefInc => None,
            Action::RefDec => self.last_ref_dec.as_ref(),
        }
    }

    pub(super) fn set_last_access(&mut self, action: Action, path_id: usize, version: &VersionVec) {
        match action {
            Action::RefInc => Access::set_or_create(&mut self.last_ref_inc, path_id, version),
            Action::RefDec => Access::set_or_create(&mut self.last_ref_dec, path_id, version),
        }
    }
}
