use crate::rt::object::Object;
use crate::rt::{self, Access, Backtrace, Synchronize, VersionVec};

use bumpalo::Bump;
use std::sync::atomic::Ordering::{Acquire, Release};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Arc {
    obj: Object,
}

#[derive(Debug)]
pub(super) struct State<'bump> {
    /// Reference count
    ref_cnt: usize,

    /// Backtrace where the arc was allocated
    allocated: Option<Backtrace>,

    /// Causality transfers between threads
    ///
    /// Only updated on on ref dec and acquired before drop
    synchronize: Synchronize<'bump>,

    /// Tracks access to the arc object
    last_ref_inc: Option<Access<'bump>>,
    last_ref_dec: Option<Access<'bump>>,
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
                allocated: execution.backtrace(),
                synchronize: Synchronize::new(execution.max_threads, execution.bump),
                last_ref_inc: None,
                last_ref_dec: None,
            });

            Arc { obj }
        })
    }

    pub(crate) fn ref_inc(self) {
        self.obj.branch(Action::RefInc);

        rt::execution(|execution| {
            let state = self.obj.arc_mut(&mut execution.objects);
            state.ref_cnt = state.ref_cnt.checked_add(1).expect("overflow");
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

            if state.ref_cnt == 1 {
                true
            } else {
                false
            }
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

impl<'bump> State<'bump> {
    pub(super) fn check_for_leaks(&self) {
        if self.ref_cnt != 0 {
            if let Some(backtrace) = &self.allocated {
                panic!(
                    "Arc leaked.\n------------\nAllocated:\n\n{}\n------------\n",
                    backtrace
                );
            } else {
                panic!("Arc leaked.");
            }
        }
    }

    pub(super) fn last_dependent_access(&self, action: Action) -> Option<&Access<'bump>> {
        match action {
            // RefIncs are not dependent w/ RefDec, only inspections
            Action::RefInc => None,
            Action::RefDec => self.last_ref_dec.as_ref(),
        }
    }

    pub(super) fn set_last_access(
        &mut self,
        action: Action,
        path_id: usize,
        version: &VersionVec<'_>,
        bump: &'bump Bump,
    ) {
        match action {
            Action::RefInc => {
                Access::set_or_create_in(&mut self.last_ref_inc, path_id, version, bump)
            }
            Action::RefDec => {
                Access::set_or_create_in(&mut self.last_ref_dec, path_id, version, bump)
            }
        }
    }
}
