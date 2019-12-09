use crate::rt;
use crate::rt::object::Object;

use tracing::trace;

/// Tracks an allocation
#[derive(Debug)]
pub(crate) struct Allocation {
    obj: Object,
}

#[derive(Debug)]
pub(super) struct State {
    is_dropped: bool,
}

/// Track a raw allocation
pub(crate) fn alloc(ptr: *mut u8) {
    rt::execution(|execution| {
        let obj = execution.objects.insert_alloc(State { is_dropped: false });

        let allocation = Allocation { obj };

        trace!(?obj, ?ptr, "alloc");

        let prev = execution.raw_allocations.insert(ptr as usize, allocation);
        assert!(prev.is_none(), "pointer already tracked");
    });
}

/// Track a raw deallocation
pub(crate) fn dealloc(ptr: *mut u8) {
    let allocation =
        rt::execution(
            |execution| match execution.raw_allocations.remove(&(ptr as usize)) {
                Some(allocation) => {
                    trace!(obj = ?allocation.obj, ?ptr, "dealloc");

                    allocation
                }
                None => panic!("pointer not tracked"),
            },
        );

    // Drop outside of the `rt::execution` block
    drop(allocation);
}

impl Allocation {
    pub(crate) fn new() -> Allocation {
        rt::execution(|execution| {
            let obj = execution.objects.insert_alloc(State { is_dropped: false });

            trace!(?obj, "Allocation::new");

            Allocation { obj }
        })
    }
}

impl Drop for Allocation {
    fn drop(&mut self) {
        rt::execution(|execution| {
            trace!(obj = ?self.obj, "Allocation::drop");

            let state = self.obj.alloc(&mut execution.objects);

            state.is_dropped = true;
        });
    }
}

impl State {
    pub(super) fn check_for_leaks(&self) {
        assert!(self.is_dropped, "object leaked");
    }
}
