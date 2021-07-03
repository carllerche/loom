#![deny(warnings, rust_2018_idioms)]

use loom::sync::atomic::AtomicUsize;
use loom::thread;

use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use std::sync::Arc;

loom::lazy_static! {
    static ref A: AtomicUsize = AtomicUsize::new(0);
    static ref NO_LEAK: loom::sync::Arc<usize> = Default::default();
    static ref ARC_WITH_SLOW_CONSTRUCTOR: loom::sync::Arc<usize> = { thread::yield_now(); Default::default() };
}

loom::thread_local! {
    static B: usize = A.load(Relaxed);
}

#[test]
#[should_panic]
fn lazy_static_arc_shutdown() {
    loom::model(|| {
        // note that we are not waiting for this thread,
        // so it may access the static during shutdown,
        // which is not okay.
        thread::spawn(|| {
            assert_eq!(**NO_LEAK, 0);
        });
    });
}

#[test]
fn lazy_static_arc_race() {
    loom::model(|| {
        let jh = thread::spawn(|| {
            assert_eq!(**ARC_WITH_SLOW_CONSTRUCTOR, 0);
        });
        assert_eq!(**ARC_WITH_SLOW_CONSTRUCTOR, 0);

        jh.join().unwrap();
    });
}

#[test]
fn lazy_static_arc_doesnt_leak() {
    loom::model(|| {
        assert_eq!(**NO_LEAK, 0);
    });
}

#[test]
fn legal_load_after_lazy_static() {
    loom::model(|| {
        let t1 = thread::spawn(|| {
            B.try_with(|h| *h).unwrap_or_else(|_| A.load(Relaxed));
        });
        let t2 = thread::spawn(|| {
            B.try_with(|h| *h).unwrap_or_else(|_| A.load(Relaxed));
        });
        t1.join().unwrap();
        t2.join().unwrap();
    });
}

#[test]
#[should_panic]
fn invalid_unsync_load_relaxed() {
    loom::model(|| {
        let a = Arc::new(AtomicUsize::new(0));
        let b = a.clone();

        let thread = thread::spawn(move || {
            unsafe { a.unsync_load() };
        });

        b.store(1, Relaxed);

        thread.join().unwrap();
    });
}

#[test]
#[ignore]
#[should_panic]
fn compare_and_swap_reads_old_values() {
    loom::model(|| {
        let a = Arc::new(AtomicUsize::new(0));
        let b = Arc::new(AtomicUsize::new(0));

        let a2 = a.clone();
        let b2 = b.clone();

        let th = thread::spawn(move || {
            a2.store(1, Release);
            b2.compare_and_swap(0, 2, AcqRel);
        });

        b.store(1, Release);
        a.compare_and_swap(0, 2, AcqRel);

        th.join().unwrap();

        let a_val = a.load(Acquire);
        let b_val = b.load(Acquire);

        if a_val == 2 && b_val == 2 {
            panic!();
        }
    });
}

#[test]
#[should_panic]
fn atomic_load_on_drop_in_panic_crashes() {
    struct AtomicLoadOnDrop(AtomicUsize);

    impl AtomicLoadOnDrop {
        fn new() -> Self {
            Self(AtomicUsize::new(0))
        }
    }

    impl Drop for AtomicLoadOnDrop {
        fn drop(&mut self) {
            let _ = self.0.load(SeqCst);
        }
    }

    loom::model(|| {
        let a = AtomicLoadOnDrop::new();

        // Moving `AtomicLoadOnDrop` into a thread will trigger `drop` when the
        // thread is dropped.
        thread::spawn(move || {
            let _a = a;
        });

        // Panic the parent thread.
        //
        // Without a fix, the `AtomicLoadOnDrop` drops _after_ the
        // `loom::rt::scheduler::STATE` thread local is dropped. Since atomics
        // use `STATE` to track accesses, dropping `AtomicLoadOnDrop` causes an
        // access to an unset `RefCell`.
        panic!();
    });
}
