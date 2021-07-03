use crate::rt::location::{self, Location, LocationSet};
use crate::rt::{self, object, thread, VersionVec};

/// Tracks immutable and mutable access to a single memory cell.
#[derive(Debug)]
pub(crate) struct Cell {
    state: object::Ref<State>,
}

/// Tracks immutable access to `Cell`
#[derive(Debug)]
pub(crate) struct CellRefToken {
    state: object::Ref<State>,
}

/// Tracks mutable access to `Cell`
#[derive(Debug)]
pub(crate) struct CellMutToken {
    state: object::Ref<State>,
}

#[derive(Debug)]
pub(super) struct State {
    /// Where the cell was created
    created_location: Location,

    /// Number of threads currently reading the cell
    is_reading: usize,

    /// `true` if in a `with_mut` closure.
    is_writing: bool,

    /// The transitive closure of all immutable accessses of `data`.
    read_access: VersionVec,

    /// Location for the *last* time a thread read from the cell.
    read_locations: LocationSet,

    /// The last mutable access of `data`.
    write_access: VersionVec,

    /// Location for the *last* time a thread wrote to the cell
    write_locations: LocationSet,
}

impl Cell {
    pub(crate) fn new(location: Location) -> Cell {
        rt::execution(|execution| {
            let state = State::new(&execution.threads, location);

            Cell {
                state: execution.objects.insert(state),
            }
        })
    }

    pub(crate) fn with<R>(&self, location: Location, f: impl FnOnce() -> R) -> R {
        // Enter the read closure
        let _token = CellRefToken::new(self.state, location);

        f()
    }

    pub(crate) fn with_mut<R>(&self, location: Location, f: impl FnOnce() -> R) -> R {
        // Enter the write closure
        let _token = CellMutToken::new(self.state, location);

        f()
    }

    pub(crate) fn guard_ref_access(&self, location: Location) -> CellRefToken {
        CellRefToken::new(self.state, location)
    }

    pub(crate) fn guard_mut_access(&self, location: Location) -> CellMutToken {
        CellMutToken::new(self.state, location)
    }
}

impl CellRefToken {
    fn new(state: object::Ref<State>, location: Location) -> Self {
        rt::synchronize(|execution| {
            let state_obj = state.get_mut(&mut execution.objects);

            assert!(!state_obj.is_writing, "currently writing to cell");

            state_obj.is_reading += 1;
            state_obj.read_locations.track(location, &execution.threads);
            state_obj.track_read(&execution.threads);

            Self { state }
        })
    }
}

impl Drop for CellRefToken {
    fn drop(&mut self) {
        rt::execution(|execution| {
            let state_obj = self.state.get_mut(&mut execution.objects);

            assert!(state_obj.is_reading > 0);
            assert!(!state_obj.is_writing);

            state_obj.is_reading -= 1;

            if !std::thread::panicking() {
                state_obj.track_read(&execution.threads);
            }
        })
    }
}

impl CellMutToken {
    fn new(state: object::Ref<State>, location: Location) -> Self {
        rt::synchronize(|execution| {
            let state_obj = state.get_mut(&mut execution.objects);

            assert!(state_obj.is_reading == 0, "currently reading from cell");
            assert!(!state_obj.is_writing, "currently writing to cell");

            state_obj.is_writing = true;
            state_obj
                .write_locations
                .track(location, &execution.threads);
            state_obj.track_write(&execution.threads);

            Self { state }
        })
    }
}

impl Drop for CellMutToken {
    fn drop(&mut self) {
        rt::execution(|execution| {
            let state_obj = self.state.get_mut(&mut execution.objects);

            assert!(state_obj.is_writing);
            assert!(state_obj.is_reading == 0);

            state_obj.is_writing = false;

            if !std::thread::panicking() {
                state_obj.track_write(&execution.threads);
            }
        })
    }
}

impl State {
    fn new(threads: &thread::Set, location: Location) -> State {
        let version = threads.active().causality.clone();

        State {
            created_location: location,
            is_reading: 0,
            is_writing: false,
            read_access: version.clone(),
            read_locations: LocationSet::new(),
            write_access: version.clone(),
            write_locations: LocationSet::new(),
        }
    }

    /// Perform a read access
    fn track_read(&mut self, threads: &thread::Set) {
        let current = &threads.active().causality;

        // Check that there is no concurrent mutable access, i.e., the last
        // mutable access must happen-before this immutable access.
        if let Some(writer) = current.ahead(&self.write_access) {
            location::panic("Causality violation: Concurrent read and write accesses.")
                .location("created", self.created_location)
                .thread("read", threads.active_id(), self.read_locations[threads])
                .thread("write", writer, self.write_locations[writer])
                .fire();
        }

        self.read_access.join(current);
    }

    fn track_write(&mut self, threads: &thread::Set) {
        let current = &threads.active().causality;

        // Check that there is no concurrent mutable access, i.e., the last
        // mutable access must happen-before this mutable access.
        if let Some(other) = current.ahead(&self.write_access) {
            location::panic("Causality violation: Concurrent write accesses to `UnsafeCell`.")
                .location("created", self.created_location)
                .thread("write one", other, self.write_locations[other])
                .thread(
                    "write two",
                    threads.active_id(),
                    self.write_locations[threads],
                )
                .fire();
        }

        // Check that there are no concurrent immutable accesss, i.e., every
        // immutable access must happen-before this mutable access.
        if let Some(reader) = current.ahead(&self.read_access) {
            location::panic(
                "Causality violation: Concurrent read and write accesses to `UnsafeCell`.",
            )
            .location("created", self.created_location)
            .thread("read", reader, self.read_locations[reader])
            .thread("write", threads.active_id(), self.write_locations[threads])
            .fire();
        }

        self.write_access.join(current);
    }
}
