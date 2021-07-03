//! Shareable mutable containers.

mod unsafe_cell;

pub use self::unsafe_cell::{UnsafeCell, UnsafeCellMutToken, UnsafeCellRefToken};
