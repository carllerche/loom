use crate::rt::object::{self, Object};
use crate::rt::{self, Access, Synchronize, VersionVec};

use std::sync::atomic::Ordering::{Acquire, Release};

use tracing::trace;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Notify {
    obj: Object,
}

#[derive(Debug)]
pub(super) struct State {
    /// If true, spurious notifications are possible
    spurious: bool,

    /// True if the notify woke up spuriously last time
    did_spur: bool,

    /// When true, notification is sequentiall consistent.
    seq_cst: bool,

    /// `true` if there is a pending notification to consume.
    notified: bool,

    /// Tracks access to the notify object
    last_access: Option<Access>,

    /// Causality transfers between threads
    synchronize: Synchronize,
}

impl Notify {
    pub(crate) fn new(seq_cst: bool, spurious: bool) -> Notify {
        super::execution(|execution| {
            let obj = execution.objects.insert_notify(State {
                spurious,
                did_spur: false,
                seq_cst,
                notified: false,
                last_access: None,
                synchronize: Synchronize::new(execution.max_threads),
            });

            trace!(?obj, ?seq_cst, ?spurious, "Notify::new");

            Notify { obj }
        })
    }

    pub(crate) fn notify(self) {
        self.obj.branch_opaque();

        rt::execution(|execution| {
            {
                let state = self.get_state(&mut execution.objects);

                state
                    .synchronize
                    .sync_store(&mut execution.threads, Release);

                if state.seq_cst {
                    execution.threads.seq_cst();
                }

                state.notified = true;
            }

            let (active, inactive) = execution.threads.split_active();

            for thread in inactive {
                let obj = thread
                    .operation
                    .as_ref()
                    .map(|operation| operation.object());

                if obj == Some(self.obj) {
                    trace!(obj = ?self.obj, thread = ?thread.id,
                           "Notify::notify");

                    thread.unpark(active);
                }
            }
        });
    }

    pub(crate) fn wait(self) {
        let (notified, spurious) = rt::execution(|execution| {
            let state = self.get_state(&mut execution.objects);

            let spurious = if state.spurious && !state.did_spur {
                execution.path.branch_spurious()
            } else {
                false
            };

            if spurious {
                state.did_spur = true;
            }

            trace!(obj = ?self.obj, notified = ?state.notified,
                   ?spurious, "Notify::wait 1");

            (state.notified, spurious)
        });

        if spurious {
            rt::yield_now();
            return;
        }

        if notified {
            self.obj.branch_opaque();
        } else {
            // This should become branch_disable
            self.obj.branch_acquire(true)
        }

        // Thread was notified
        super::execution(|execution| {
            trace!(obj = ?self.obj, "Notify::wait 2");

            let state = self.get_state(&mut execution.objects);

            assert!(state.notified);

            state.synchronize.sync_load(&mut execution.threads, Acquire);

            if state.seq_cst {
                // Establish sequential consistency between locks
                execution.threads.seq_cst();
            }

            state.notified = false;
        });
    }

    fn get_state<'a>(self, store: &'a mut object::Store) -> &'a mut State {
        self.obj.notify_mut(store).unwrap()
    }
}

impl State {
    pub(crate) fn last_dependent_access(&self) -> Option<&Access> {
        self.last_access.as_ref()
    }

    pub(crate) fn set_last_access(&mut self, path_id: usize, version: &VersionVec) {
        Access::set_or_create(&mut self.last_access, path_id, version);
    }
}
