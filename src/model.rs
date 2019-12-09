//! Model concurrent programs.

use crate::rt::{self, Execution, Scheduler};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{info, subscriber};
use tracing_subscriber::{fmt, EnvFilter};

const DEFAULT_MAX_THREADS: usize = 4;
const DEFAULT_MAX_BRANCHES: usize = 1_000;

/// Configure a model
#[derive(Debug)]
pub struct Builder {
    /// Max number of threads to check as part of the execution. This should be set as low as possible.
    pub max_threads: usize,

    /// Maximum number of thread switches per permutation.
    pub max_branches: usize,

    /// Maximum number of permutations to explore.
    pub max_permutations: Option<usize>,

    /// Maximum amount of time to spend on checking
    pub max_duration: Option<Duration>,

    /// Maximum number of thread preemptions to explore
    pub preemption_bound: Option<usize>,

    /// When doing an exhaustive check, uses the file to store and load the
    /// check progress
    pub checkpoint_file: Option<PathBuf>,

    /// How often to write the checkpoint file
    pub checkpoint_interval: usize,

    // Support adding more fields in the future
    _p: (),
}

impl Builder {
    /// Create a new `Builder` instance with default values.
    pub fn new() -> Builder {
        use std::env;

        let checkpoint_interval = env::var("LOOM_CHECKPOINT_INTERVAL")
            .map(|v| {
                v.parse()
                    .ok()
                    .expect("invalid value for `LOOM_CHECKPOINT_INTERVAL`")
            })
            .unwrap_or(20_000);

        let max_branches = env::var("LOOM_MAX_BRANCHES")
            .map(|v| {
                v.parse()
                    .ok()
                    .expect("invalid value for `LOOM_MAX_BRANCHES`")
            })
            .unwrap_or(DEFAULT_MAX_BRANCHES);

        let max_duration = env::var("LOOM_MAX_DURATION")
            .map(|v| {
                let secs = v
                    .parse()
                    .ok()
                    .expect("invalid value for `LOOM_MAX_DURATION`");
                Duration::from_secs(secs)
            })
            .ok();

        let max_permutations = env::var("LOOM_MAX_PERMUTATIONS")
            .map(|v| {
                v.parse()
                    .ok()
                    .expect("invalid value for `LOOM_MAX_PERMUTATIONS`")
            })
            .ok();

        let preemption_bound = env::var("LOOM_MAX_PREEMPTIONS")
            .map(|v| {
                v.parse()
                    .ok()
                    .expect("invalid value for `LOOM_MAX_PREEMPTIONS`")
            })
            .ok();

        let checkpoint_file = env::var("LOOM_CHECKPOINT_FILE")
            .map(|v| {
                v.parse()
                    .ok()
                    .expect("invalid value for `LOOM_CHECKPOINT_FILE`")
            })
            .ok();

        Builder {
            max_threads: DEFAULT_MAX_THREADS,
            max_branches,
            max_duration,
            max_permutations,
            preemption_bound,
            checkpoint_file,
            checkpoint_interval,
            _p: (),
        }
    }

    /// Set the checkpoint file.
    pub fn checkpoint_file(&mut self, file: &str) -> &mut Self {
        self.checkpoint_file = Some(file.into());
        self
    }

    /// CHeck a model
    pub fn check<F>(&self, f: F)
    where
        F: Fn() + Sync + Send + 'static,
    {
        let mut execution =
            Execution::new(self.max_threads, self.max_branches, self.preemption_bound);
        let mut scheduler = Scheduler::new(self.max_threads);

        if let Some(ref path) = self.checkpoint_file {
            if path.exists() {
                execution.path = checkpoint::load_execution_path(path);
            }
        }

        let f = Arc::new(f);

        let mut i = 0;

        let start = Instant::now();

        loop {
            i += 1;

            if i % self.checkpoint_interval == 0 {
                info!("");
                info!(" ================== Iteration {} ==================", i);
                info!("");

                if let Some(ref path) = self.checkpoint_file {
                    checkpoint::store_execution_path(&execution.path, path);
                }

                if let Some(max_permutations) = self.max_permutations {
                    if i >= max_permutations {
                        return;
                    }
                }

                if let Some(max_duration) = self.max_duration {
                    if start.elapsed() >= max_duration {
                        return;
                    }
                }
            }

            let f = f.clone();

            scheduler.run(&mut execution, move || {
                f();
                rt::thread_done();
            });

            execution.check_for_leaks();

            if let Some(next) = execution.step() {
                execution = next;
            } else {
                info!("Completed in {} iterations", i);
                return;
            }
        }
    }
}

/// Run all concurrent permutations of the provided closure.
pub fn model<F>(f: F)
where
    F: Fn() + Sync + Send + 'static,
{
    let subscriber = fmt::Subscriber::builder()
        .with_env_filter(EnvFilter::from_env("LOOM_LOG"))
        .finish();

    subscriber::with_default(subscriber, || {
        Builder::new().check(f);
    });
}

#[cfg(feature = "checkpoint")]
mod checkpoint {
    use serde_json;
    use std::fs::File;
    use std::io::prelude::*;
    use std::path::Path;

    pub(crate) fn load_execution_path(fs_path: &Path) -> crate::rt::Path {
        let mut file = File::open(fs_path).unwrap();
        let mut contents = String::new();
        file.read_to_string(&mut contents).unwrap();
        serde_json::from_str(&contents).unwrap()
    }

    pub(crate) fn store_execution_path(path: &crate::rt::Path, fs_path: &Path) {
        let serialized = serde_json::to_string(path).unwrap();

        let mut file = File::create(fs_path).unwrap();
        file.write_all(serialized.as_bytes()).unwrap();
    }
}

#[cfg(not(feature = "checkpoint"))]
mod checkpoint {
    use std::path::Path;

    pub(crate) fn load_execution_path(_fs_path: &Path) -> crate::rt::Path {
        panic!("not compiled with `checkpoint` feature")
    }

    pub(crate) fn store_execution_path(_path: &crate::rt::Path, _fs_path: &Path) {
        panic!("not compiled with `checkpoint` feature")
    }
}
