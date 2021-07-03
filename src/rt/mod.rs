#[macro_use]
mod location;
pub(crate) use self::location::Location;

mod access;
use self::access::Access;

mod alloc;
pub(crate) use self::alloc::{alloc, dealloc, Allocation};

mod arc;
pub(crate) use self::arc::Arc;

mod atomic;
pub(crate) use self::atomic::{fence, Atomic};

mod cell;
pub(crate) use self::cell::Cell;

mod condvar;
pub(crate) use self::condvar::Condvar;

mod execution;
pub(crate) use self::execution::Execution;

mod notify;
pub(crate) use self::notify::Notify;

mod num;
pub(crate) use self::num::Numeric;

#[macro_use]
pub(crate) mod object;

mod mpsc;
pub(crate) use self::mpsc::Channel;

mod mutex;
pub(crate) use self::mutex::Mutex;

mod path;
pub(crate) use self::path::Path;

mod rwlock;
pub(crate) use self::rwlock::RwLock;

mod scheduler;
pub(crate) use self::scheduler::Scheduler;

mod synchronize;
pub(crate) use self::synchronize::Synchronize;

pub(crate) mod lazy_static;
pub(crate) mod thread;

mod vv;
pub(crate) use self::vv::VersionVec;

/// Maximum number of threads that can be included in a model.
pub const MAX_THREADS: usize = 4;

/// Maximum number of atomic store history to track per-cell.
pub(crate) const MAX_ATOMIC_HISTORY: usize = 7;

pub(crate) fn spawn<F>(f: F) -> crate::rt::thread::Id
where
    F: FnOnce() + 'static,
{
    let id = execution(|execution| execution.new_thread());

    Scheduler::spawn(Box::new(move || {
        f();
        thread_done();
    }));

    id
}

/// Marks the current thread as blocked
pub fn park() {
    execution(|execution| {
        execution.threads.active_mut().set_blocked();
        execution.threads.active_mut().operation = None;
        execution.schedule()
    });

    Scheduler::switch();
}

/// Add an execution branch point.
fn branch<F, R>(f: F) -> R
where
    F: FnOnce(&mut Execution) -> R,
{
    let (ret, switch) = execution(|execution| {
        let ret = f(execution);
        (ret, execution.schedule())
    });

    if switch {
        Scheduler::switch();
    }

    ret
}

fn synchronize<F, R>(f: F) -> R
where
    F: FnOnce(&mut Execution) -> R,
{
    execution(|execution| {
        execution.threads.active_causality_inc();
        let ret = f(execution);
        ret
    })
}

/// Yield the thread.
///
/// This enables concurrent algorithms that require other threads to make
/// progress.
pub fn yield_now() {
    let switch = execution(|execution| {
        execution.threads.active_mut().set_yield();
        execution.threads.active_mut().operation = None;
        execution.schedule()
    });

    if switch {
        Scheduler::switch();
    }
}

pub(crate) fn execution<F, R>(f: F) -> R
where
    F: FnOnce(&mut Execution) -> R,
{
    Scheduler::with_execution(f)
}

pub fn thread_done() {
    let locals = execution(|execution| execution.threads.active_mut().drop_locals());

    // Drop outside of the execution context
    drop(locals);

    execution(|execution| {
        execution.threads.active_mut().operation = None;
        execution.threads.active_mut().set_terminated();
        execution.schedule();
    });
}
