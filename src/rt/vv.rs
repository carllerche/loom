use crate::rt::{execution, thread};
use bumpalo::Bump;

use std::cmp;
use std::ops;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct VersionVec<'bump> {
    versions: &'bump mut [usize],
}

impl VersionVec<'_> {
    pub(crate) fn new_in(max_threads: usize, bump: &Bump) -> VersionVec<'_> {
        VersionVec {
            versions: bump.alloc_slice_fill_copy(max_threads, 0),
        }
    }

    pub(crate) fn clone_in<'bump>(&self, bump: &'bump Bump) -> VersionVec<'bump> {
        VersionVec {
            versions: bump.alloc_slice_copy(&self.versions),
        }
    }

    pub(crate) fn set(&mut self, other: &VersionVec<'_>) {
        self.versions.copy_from_slice(&other.versions);
    }

    pub(crate) fn versions<'a>(
        &'a self,
        execution_id: execution::Id,
    ) -> impl Iterator<Item = (thread::Id, usize)> + 'a {
        self.versions
            .iter()
            .enumerate()
            .map(move |(thread_id, &version)| (thread::Id::new(execution_id, thread_id), version))
    }

    pub(crate) fn len(&self) -> usize {
        self.versions.len()
    }

    pub(crate) fn inc(&mut self, id: thread::Id) {
        self.versions[id.as_usize()] += 1;
    }

    pub(crate) fn join(&mut self, other: &VersionVec<'_>) {
        assert_eq!(self.versions.len(), other.versions.len());

        for (i, &version) in other.versions.iter().enumerate() {
            self.versions[i] = cmp::max(self.versions[i], version);
        }
    }
}

impl cmp::PartialOrd for VersionVec<'_> {
    fn partial_cmp(&self, other: &VersionVec<'_>) -> Option<cmp::Ordering> {
        use cmp::Ordering::*;

        let len = cmp::max(self.len(), other.len());
        let mut ret = Equal;

        for i in 0..len {
            let a = self.versions.get(i).unwrap_or(&0);
            let b = other.versions.get(i).unwrap_or(&0);

            if a == b {
                // Keep checking
            } else if a < b {
                if ret == Greater {
                    return None;
                }

                ret = Less;
            } else {
                if ret == Less {
                    return None;
                }

                ret = Greater;
            }
        }

        Some(ret)
    }
}

impl ops::Index<thread::Id> for VersionVec<'_> {
    type Output = usize;

    fn index(&self, index: thread::Id) -> &usize {
        self.versions.index(index.as_usize())
    }
}

impl ops::IndexMut<thread::Id> for VersionVec<'_> {
    fn index_mut(&mut self, index: thread::Id) -> &mut usize {
        self.versions.index_mut(index.as_usize())
    }
}
