//! Future related synchronization primitives.

mod atomic_waker;

pub use self::atomic_waker::AtomicWaker;

use crate::rt;
use crate::sync::Arc;

use futures_util::pin_mut;
use std::future::Future;
use std::mem;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

/// Block the current thread, driving `f` to completion.
#[track_caller]
pub fn block_on<F>(f: F) -> F::Output
where
    F: Future,
{
    pin_mut!(f);

    let notify = Arc::new(rt::Notify::new(false, true));

    let mut waker = unsafe {
        mem::ManuallyDrop::new(Waker::from_raw(RawWaker::new(
            &*notify as *const _ as *const (),
            waker_vtable(),
        )))
    };

    let mut cx = Context::from_waker(&mut waker);

    loop {
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(val) => return val,
            Poll::Pending => {}
        }

        notify.wait(&trace!());
    }
}

pub(super) fn waker_vtable() -> &'static RawWakerVTable {
    &RawWakerVTable::new(
        clone_arc_raw,
        wake_arc_raw,
        wake_by_ref_arc_raw,
        drop_arc_raw,
    )
}

unsafe fn increase_refcount(data: *const ()) {
    // Retain Arc, but don't touch refcount by wrapping in ManuallyDrop
    let arc = mem::ManuallyDrop::new(Arc::<rt::Notify>::from_raw(data as *const _));
    // Now increase refcount, but don't drop new refcount either
    let _arc_clone: mem::ManuallyDrop<_> = arc.clone();
}

unsafe fn clone_arc_raw(data: *const ()) -> RawWaker {
    increase_refcount(data);
    RawWaker::new(data, waker_vtable())
}

#[track_caller]
unsafe fn wake_arc_raw(data: *const ()) {
    let notify: Arc<rt::Notify> = Arc::from_raw(data as *const _);
    notify.notify(&trace!());
}

#[track_caller]
unsafe fn wake_by_ref_arc_raw(data: *const ()) {
    // Retain Arc, but don't touch refcount by wrapping in ManuallyDrop
    let arc = mem::ManuallyDrop::new(Arc::<rt::Notify>::from_raw(data as *const _));
    arc.notify(&trace!());
}

unsafe fn drop_arc_raw(data: *const ()) {
    drop(Arc::<rt::Notify>::from_raw(data as *const _))
}
