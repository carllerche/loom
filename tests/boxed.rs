#![deny(warnings, rust_2018_idioms)]

use loom::boxed::Box;
use loom::sync::atomic::{AtomicU8, Ordering};
use loom::sync::Arc;
use std::mem;

#[derive(Clone)]
struct DetectDrop(Arc<AtomicU8>);

impl DetectDrop {
    pub fn new() -> (Self, Arc<AtomicU8>) {
        let drop_count = Arc::new(AtomicU8::new(0));
        (Self(drop_count.clone()), drop_count)
    }
}

impl Drop for DetectDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn allocate_and_drop() {
    loom::model(|| {
        let (detect_drop, drop_count) = DetectDrop::new();
        let detect_drop = Box::new(detect_drop);
        assert_eq!(drop_count.load(Ordering::SeqCst), 0);
        mem::drop(detect_drop);
        assert_eq!(drop_count.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn allocate_via_raw_and_drop() {
    loom::model(|| {
        let (detect_drop, drop_count) = DetectDrop::new();
        let detect_drop = Box::new(detect_drop);
        assert_eq!(drop_count.load(Ordering::SeqCst), 0);
        let detect_drop_ptr: *mut DetectDrop = Box::into_raw(detect_drop);
        assert_eq!(drop_count.load(Ordering::SeqCst), 0);
        let detect_drop = unsafe { Box::from_raw(detect_drop_ptr) };
        assert_eq!(drop_count.load(Ordering::SeqCst), 0);
        mem::drop(detect_drop);
        assert_eq!(drop_count.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn into_value() {
    loom::model(|| {
        let (detect_drop, drop_count) = DetectDrop::new();
        let detect_drop = Box::new(detect_drop);
        let value: DetectDrop = detect_drop.into_value();
        assert_eq!(drop_count.load(Ordering::SeqCst), 0);
        mem::drop(value);
        assert_eq!(drop_count.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn clone() {
    loom::model(|| {
        let (detect_drop, drop_count) = DetectDrop::new();
        let detect_drop = Box::new(detect_drop);
        let detect_drop2: Box<DetectDrop> = detect_drop.clone();
        assert_eq!(drop_count.load(Ordering::SeqCst), 0);
        mem::drop(detect_drop);
        assert_eq!(drop_count.load(Ordering::SeqCst), 1);
        mem::drop(detect_drop2);
        assert_eq!(drop_count.load(Ordering::SeqCst), 2);
    });
}

#[test]
#[should_panic]
#[ignore]
fn allocate_and_leak() {
    loom::model(|| {
        let (detect_drop, _detect_drop) = DetectDrop::new();
        let detect_drop = Box::new(detect_drop);
        Box::into_raw(detect_drop);
    });
}

#[test]
fn same_size_as_std_box() {
    use std::boxed::Box as StdBox;

    macro_rules! same_size_and_alignment {
        ($t:ty) => {
            assert_eq!(
                mem::size_of::<Box<$t>>(),
                mem::size_of::<StdBox<$t>>(),
                "size of Box<{}>",
                stringify!($t),
            );
            assert_eq!(
                mem::align_of::<Box<$t>>(),
                mem::align_of::<StdBox<$t>>(),
                "align of Box<{}>",
                stringify!($t),
            );
            assert_eq!(
                mem::size_of::<Option<Box<$t>>>(),
                mem::size_of::<Option<StdBox<$t>>>(),
                "size of Option<Box<{}>>",
                stringify!($t),
            );
            assert_eq!(
                mem::align_of::<Option<Box<$t>>>(),
                mem::align_of::<Option<StdBox<$t>>>(),
                "align of Option<Box<{}>>",
                stringify!($t),
            );
        };
    }

    same_size_and_alignment!(std::convert::Infallible);
    same_size_and_alignment!(());
    same_size_and_alignment!(u8);
    same_size_and_alignment!([u32; 1024]);
}
