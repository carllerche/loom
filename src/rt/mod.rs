mod access;
use self::access::Access;

mod alloc;
pub(crate) use self::alloc::{alloc, dealloc, Allocation};

mod arc;
pub(crate) use self::arc::Arc;

mod atomic;
pub(crate) use self::atomic::{fence, Atomic};

mod backtrace;
pub(crate) use self::backtrace::Backtrace;

mod condvar;
pub(crate) use self::condvar::Condvar;

mod execution;
pub(crate) use self::execution::Execution;

mod notify;
pub(crate) use self::notify::Notify;

pub(crate) mod object;

mod mutex;
pub(crate) use self::mutex::Mutex;

mod causal;
pub(crate) use self::causal::{CausalCell, CausalCheck};

mod path;
pub(crate) use self::path::Path;

mod scheduler;
pub(crate) use self::scheduler::Scheduler;

mod synchronize;
pub(crate) use self::synchronize::Synchronize;

pub(crate) mod thread;

mod vv;
pub(crate) use self::vv::VersionVec;

pub fn spawn<F>(f: F)
where
    F: FnOnce() + 'static,
{
    execution(|execution| {
        execution.new_thread();
    });

    Scheduler::spawn(Box::new(move || {
        f();
        thread_done();
    }));
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
    F: FnOnce(&mut Execution<'_>) -> R,
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
    F: FnOnce(&mut Execution<'_>) -> R,
{
    execution(|execution| {
        let ret = f(execution);
        execution.threads.active_causality_inc();
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

/// Critical section, may not branch.
pub fn critical<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    struct Reset;

    impl Drop for Reset {
        fn drop(&mut self) {
            execution(|execution| {
                execution.unset_critical();
            });
        }
    }

    let _reset = Reset;

    execution(|execution| {
        execution.set_critical();
    });

    f()
}

pub(crate) fn execution<F, R>(f: F) -> R
where
    F: FnOnce(&mut Execution<'_>) -> R,
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
