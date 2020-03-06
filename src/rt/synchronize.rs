use crate::rt::{thread, VersionVec};

use bumpalo::Bump;
use std::sync::atomic::Ordering::{self, *};

/// A synchronization point between two threads.
///
/// Threads synchronize with this point using any of the available orderings. On
/// loads, the thread's causality is updated using the synchronization point's
/// stored causality. On stores, the synchronization point's causality is
/// updated with the threads.
#[derive(Debug)]
pub(crate) struct Synchronize<'bump> {
    happens_before: VersionVec<'bump>,
}

impl<'bump> Synchronize<'bump> {
    pub fn new(max_threads: usize, bump: &'bump Bump) -> Self {
        let happens_before = VersionVec::new_in(max_threads, bump);

        Synchronize { happens_before }
    }

    pub fn clone_in(&self, bump: &'bump Bump) -> Self {
        let mut res = Self::new(self.happens_before.len(), bump);
        res.happens_before.set(&self.happens_before);
        res
    }

    pub fn version_vec(&self) -> &VersionVec<'_> {
        &self.happens_before
    }

    pub fn sync_load(&mut self, threads: &mut thread::Set<'_>, order: Ordering) {
        match order {
            Relaxed | Release => {
                // Nothing happens!
            }
            Acquire | AcqRel => {
                self.sync_acq(threads);
            }
            SeqCst => {
                self.sync_acq(threads);
                threads.seq_cst();
            }
            order => unimplemented!("unimplemented ordering {:?}", order),
        }
    }

    pub fn sync_store(&mut self, threads: &mut thread::Set<'_>, order: Ordering) {
        match order {
            Relaxed | Acquire => {
                // Nothing happens!
            }
            Release | AcqRel => {
                self.sync_rel(threads);
            }
            SeqCst => {
                self.sync_rel(threads);
                threads.seq_cst();
            }
            order => unimplemented!("unimplemented ordering {:?}", order),
        }
    }

    fn sync_acq(&mut self, threads: &mut thread::Set<'_>) {
        threads.active_mut().causality.join(&self.happens_before);
    }

    fn sync_rel(&mut self, threads: &thread::Set<'_>) {
        self.happens_before.join(&threads.active().causality);
    }
}
