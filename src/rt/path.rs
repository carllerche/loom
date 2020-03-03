use crate::rt::{execution, thread};

#[cfg(feature = "checkpoint")]
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// An execution path
#[derive(Debug)]
#[cfg_attr(feature = "checkpoint", derive(Serialize, Deserialize))]
pub(crate) struct Path {
    preemption_bound: Option<usize>,

    /// Current execution's position in the branch index.
    ///
    /// When the execution starts, this is zero, but `branches` might not be
    /// empty.
    ///
    /// In order to perform an exhaustive search, the execution is seeded with a
    /// set of branches.
    pos: usize,

    /// Sequence of all decisions in a loom execution that can be permuted.
    ///
    /// This vec tracks the branch kind and index into one of the vecs below.
    /// Each branch kind is tracked separately to make backtracking algorithms
    /// simpler.
    branches: Vec<Branch>,

    /// Tracks threads to be scheduled
    schedules: Vec<Schedule>,
    thread_states: SliceVec<Thread>,

    /// Atomic writes
    writes: SliceVec<usize>,
    /// i-th element in `current_writes` tracks current position in the i-th slice in `writes`
    current_writes: Vec<usize>,

    /// Tracks spurious notifications
    spurious: Vec<VecDeque<bool>>,

    /// Maximum number of branches to explore
    max_branches: usize,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "checkpoint", derive(Serialize, Deserialize))]
enum Branch {
    Schedule(usize),
    Write(usize),
    Spurious(usize),
}

#[derive(Debug)]
#[cfg_attr(feature = "checkpoint", derive(Serialize, Deserialize))]
pub(crate) struct Schedule {
    pub(crate) preemptions: usize,

    pub(crate) initial_active: Option<usize>,

    current_active: Option<usize>,
}

/// A container that is conceptually equivalent to Vec<[T]>.
/// It allows storing slices of differing sizes in a single contiguous chunk of memory
/// with random access.
#[derive(Debug)]
#[cfg_attr(feature = "checkpoint", derive(Serialize, Deserialize))]
pub(crate) struct SliceVec<T> {
    data: Vec<T>,
    slice_ends: Vec<usize>,
}

#[derive(Debug, Eq, PartialEq, Clone)]
#[cfg_attr(feature = "checkpoint", derive(Serialize, Deserialize))]
pub(crate) enum Thread {
    /// The thread is currently disabled
    Disabled,

    /// The thread should not be explored
    Skip,

    /// The thread is in a yield state.
    Yield,

    /// The thread is waiting to be explored
    Pending,

    /// The thread is currently being explored
    Active,

    /// The thread has been explored
    Visited,
}

impl Path {
    /// New Path
    pub(crate) fn new(max_branches: usize, preemption_bound: Option<usize>) -> Path {
        Path {
            preemption_bound,
            branches: vec![],
            pos: 0,
            schedules: vec![],
            thread_states: SliceVec::new(),
            writes: SliceVec::new(),
            current_writes: vec![],
            spurious: vec![],
            max_branches,
        }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    /// Returns the atomic write to read
    pub(crate) fn branch_write<I>(&mut self, seed: I) -> usize
    where
        I: Iterator<Item = usize>,
    {
        use self::Branch::Write;

        assert!(
            self.branches.len() < self.max_branches,
            "actual = {}",
            self.branches.len()
        );

        if self.pos == self.branches.len() {
            let i = self.writes.len();
            self.writes.extend(seed);
            self.current_writes.push(0);
            self.branches.push(Branch::Write(i));
        }

        let i = match self.branches[self.pos] {
            Write(i) => i,
            _ => panic!("path entry {} is not a write", self.pos),
        };

        self.pos += 1;

        self.writes.get(i)[self.current_writes[i]]
    }

    /// Branch on spurious notifications
    pub(crate) fn branch_spurious(&mut self) -> bool {
        use self::Branch::Spurious;

        assert!(
            self.branches.len() < self.max_branches,
            "actual = {}",
            self.branches.len()
        );

        if self.pos == self.branches.len() {
            let i = self.spurious.len();

            let spurious: VecDeque<_> = vec![false, true].into();
            self.spurious.push(spurious);
            self.branches.push(Branch::Spurious(i));
        }

        let i = match self.branches[self.pos] {
            Spurious(i) => i,
            _ => panic!("path entry {} is not a spurious wait", self.pos),
        };

        self.pos += 1;
        self.spurious[i][0]
    }

    /// Returns the thread identifier to schedule
    pub(crate) fn branch_thread<I>(
        &mut self,
        execution_id: execution::Id,
        seed: I,
    ) -> Option<thread::Id>
    where
        I: Iterator<Item = Thread>,
    {
        assert!(
            self.branches.len() < self.max_branches,
            "actual = {}",
            self.branches.len()
        );

        if self.pos == self.branches.len() {
            // Entering a new exploration space.

            let i = self.schedules.len();

            self.thread_states.extend(seed);
            let threads = self.thread_states.get_mut(i);

            let mut current_active = None;

            for (i, th) in threads.iter().enumerate() {
                if th.is_active() {
                    assert!(current_active.is_none(), "more than one active thread");
                    current_active = Some(i);
                }
            }

            // Ensure at least one thread is active, otherwise toggle a yielded
            // thread.
            if current_active.is_none() {
                for (i, th) in threads.iter_mut().enumerate() {
                    if *th == Thread::Yield {
                        assert!(current_active.is_none(), "more than one yielded thread");
                        *th = Thread::Active;
                        current_active = Some(i);
                    }
                }
            }

            let initial_active = if let Some(prev) = self.schedules.last() {
                if current_active == prev.current_active {
                    current_active
                } else {
                    None
                }
            } else {
                current_active
            };

            let preemptions = if let Some(prev) = self.schedules.last() {
                let mut preemptions = prev.preemptions;

                if prev.initial_active.is_some() && prev.initial_active != prev.current_active {
                    preemptions += 1;
                }

                preemptions
            } else {
                0
            };

            self.schedules.push(Schedule {
                preemptions,
                initial_active,
                current_active,
            });

            self.branches.push(Branch::Schedule(i));
        }

        let i = match self.branches[self.pos] {
            Branch::Schedule(i) => i,
            _ => panic!(),
        };

        self.pos += 1;
        self.schedules[i]
            .current_active
            .map(|id| thread::Id::new(execution_id, id))
    }

    pub(crate) fn backtrack(&mut self, point: usize, thread_id: thread::Id) {
        let index = match self.branches[point] {
            Branch::Schedule(index) => index,
            _ => panic!(),
        };

        // Exhaustive DPOR only requires adding this backtrack point
        self.schedules[index].backtrack(
            thread_id,
            self.preemption_bound,
            self.thread_states.get_mut(index),
        );

        if self.preemption_bound.is_some() {
            if index > 0 {
                for j in (1..index).rev() {
                    // Preemption bounded DPOR requires conservatively adding another
                    // backtrack point to cover cases missed by the bounds.
                    if self.schedules[j].current_active != self.schedules[j - 1].current_active {
                        self.schedules[j].backtrack(
                            thread_id,
                            self.preemption_bound,
                            self.thread_states.get_mut(j),
                        );
                        return;
                    }
                }

                self.schedules[0].backtrack(
                    thread_id,
                    self.preemption_bound,
                    self.thread_states.get_mut(0),
                );
            }
        }
    }

    /// Returns `false` if there are no more paths to explore
    pub(crate) fn step(&mut self) -> bool {
        use self::Branch::*;

        self.pos = 0;

        while self.branches.len() > 0 {
            match self.branches.last().unwrap() {
                &Schedule(i) => {
                    let schedule = &mut self.schedules[i];
                    let threads = self.thread_states.get_mut(i);

                    // Transition the active thread to visited.
                    if let Some(active_id) = schedule.current_active.take() {
                        threads[active_id] = Thread::Visited;
                    }

                    // Find a pending thread and transition it to active
                    for (i, th) in threads.iter_mut().enumerate() {
                        if th.is_pending() {
                            *th = Thread::Active;
                            schedule.current_active = Some(i);
                            break;
                        }
                    }

                    if schedule.current_active.is_none() {
                        self.branches.pop();
                        self.schedules.pop();
                        self.thread_states.pop();
                        continue;
                    }
                }
                &Write(i) => {
                    self.current_writes[i] += 1;

                    if self.current_writes[i] == self.writes.get(i).len() {
                        self.branches.pop();
                        self.writes.pop();
                        self.current_writes.pop();
                        continue;
                    }
                }
                &Spurious(i) => {
                    self.spurious[i].pop_front();

                    if self.spurious[i].is_empty() {
                        self.branches.pop();
                        self.spurious.pop();
                        continue;
                    }
                }
            }

            return true;
        }

        false
    }
}

impl Schedule {
    fn backtrack(
        &mut self,
        thread_id: thread::Id,
        preemption_bound: Option<usize>,
        threads: &mut [Thread],
    ) {
        if let Some(bound) = preemption_bound {
            assert!(self.preemptions <= bound, "actual = {}", self.preemptions);

            if self.preemptions == bound {
                return;
            }
        }

        let thread_id = thread_id.as_usize();

        if thread_id >= threads.len() {
            return;
        }

        if threads[thread_id].is_enabled() {
            threads[thread_id].explore();
        } else {
            for th in threads {
                th.explore();
            }
        }
    }
}

impl<T> SliceVec<T> {
    fn new() -> Self {
        Self {
            data: vec![],
            slice_ends: vec![],
        }
    }

    fn extend(&mut self, it: impl Iterator<Item = T>) {
        self.data.extend(it);
        self.slice_ends.push(self.data.len());
    }

    fn pop(&mut self) {
        if self.slice_ends.pop().is_none() {
            return;
        }

        let new_len = self.slice_ends.last().unwrap_or(&0);
        self.data.truncate(*new_len);
    }

    fn len(&self) -> usize {
        self.slice_ends.len()
    }

    fn get(&self, i: usize) -> &[T] {
        let slice_begin = if i == 0 { 0 } else { self.slice_ends[i - 1] };
        &self.data[slice_begin..self.slice_ends[i]]
    }

    fn get_mut(&mut self, i: usize) -> &mut [T] {
        let slice_begin = if i == 0 { 0 } else { self.slice_ends[i - 1] };
        &mut self.data[slice_begin..self.slice_ends[i]]
    }
}

impl Thread {
    fn explore(&mut self) {
        match *self {
            Thread::Skip => {
                *self = Thread::Pending;
            }
            _ => {}
        }
    }

    fn is_pending(&self) -> bool {
        match *self {
            Thread::Pending => true,
            _ => false,
        }
    }

    fn is_active(&self) -> bool {
        match *self {
            Thread::Active => true,
            _ => false,
        }
    }

    fn is_enabled(&self) -> bool {
        !self.is_disabled()
    }

    fn is_disabled(&self) -> bool {
        *self == Thread::Disabled
    }
}
