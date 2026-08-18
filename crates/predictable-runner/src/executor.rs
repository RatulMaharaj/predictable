//! Where chunks run (`03-engine.md` §5.1, §10).
//!
//! The engine is a pure function of one chunk; *parallelism is a partition of the modelpoint
//! set*, and it lives here. [`ChunkExecutor`] is the seam `03-engine.md` §10 asks for: today it
//! has a serial implementation and a rayon-backed [`LocalExecutor`], and a distributed one
//! slots in behind the same trait without the kernel knowing.
//!
//! Three rules make the seam safe:
//!
//! 1. **Results come back in chunk index order**, never completion order. Every implementation
//!    returns a `Vec` indexed by chunk position, so `results.parquet` and the aggregation fold
//!    are bit-identical whatever the scheduler does (`01-ir.md` §9.1).
//! 2. **One worker per thread, made by a factory.** A worker owns an [`predictable_engine::Engine`]
//!    and its arena; nothing is shared mutably, so there is no lock in the hot path and no
//!    cross-thread dictionary.
//! 3. **Cancellation is checked between chunks** ([`crate::CancelFlag`]). A cancelled chunk is
//!    reported as [`ChunkResult::Cancelled`] rather than silently missing.

use rayon::prelude::*;

use predictable_engine::{ChunkOutput, EngineError};

use crate::cancel::CancelFlag;
use crate::chunk::Chunk;

/// One thread's projection state: an engine, its arena, and whatever else it needs.
pub trait ChunkWorker: Send {
    /// Project one chunk.
    fn run(&mut self, chunk: &Chunk) -> Result<ChunkOutput, EngineError>;
}

/// Makes a fresh [`ChunkWorker`] for a thread. Called once per worker, never per chunk.
pub type WorkerFactory<'w> = dyn Fn() -> Box<dyn ChunkWorker + 'w> + Sync + 'w;

/// What became of one chunk.
#[derive(Debug)]
pub enum ChunkResult {
    /// The chunk projected.
    Done(Box<ChunkOutput>),
    /// The chunk trapped under `on_trap = "abort"`, or failed to bind.
    Failed(EngineError),
    /// The cancel flag was set before this chunk started (`03-engine.md` §10).
    Cancelled,
}

impl ChunkResult {
    /// The output, if the chunk completed.
    pub fn output(&self) -> Option<&ChunkOutput> {
        match self {
            ChunkResult::Done(o) => Some(o),
            _ => None,
        }
    }

    /// True for [`ChunkResult::Cancelled`].
    pub fn is_cancelled(&self) -> bool {
        matches!(self, ChunkResult::Cancelled)
    }
}

/// Runs chunks. The only thing in the system that decides *where* a chunk is projected.
pub trait ChunkExecutor: Send + Sync {
    /// Project every chunk, returning one result per chunk **in chunk index order**.
    fn map_chunks<'w>(
        &self,
        chunks: &[Chunk],
        make_worker: &WorkerFactory<'w>,
        cancel: &CancelFlag,
    ) -> Vec<ChunkResult>;

    /// How many workers this executor will make. Recorded in `manifest.run_config.exec.threads`.
    fn threads(&self) -> usize;

    /// A name for the manifest and the CLI's progress line.
    fn name(&self) -> &'static str;
}

/// One worker, one thread, chunks in order. The reference implementation: every other executor
/// must produce byte-identical results to this one.
#[derive(Debug, Clone, Copy, Default)]
pub struct SerialExecutor;

impl ChunkExecutor for SerialExecutor {
    fn map_chunks<'w>(
        &self,
        chunks: &[Chunk],
        make_worker: &WorkerFactory<'w>,
        cancel: &CancelFlag,
    ) -> Vec<ChunkResult> {
        let mut worker = make_worker();
        chunks
            .iter()
            .map(|chunk| run_one(&mut *worker, chunk, cancel))
            .collect()
    }

    fn threads(&self) -> usize {
        1
    }

    fn name(&self) -> &'static str {
        "serial"
    }
}

/// Rayon over the local machine: `threads` workers, chunks stolen in any order, results
/// reassembled in chunk index order by `map_init`'s indexed collect.
#[derive(Debug, Clone, Copy)]
pub struct LocalExecutor {
    threads: usize,
}

impl Default for LocalExecutor {
    fn default() -> LocalExecutor {
        LocalExecutor::new(0)
    }
}

impl LocalExecutor {
    /// `threads = 0` means "as many as rayon's global pool has".
    pub fn new(threads: usize) -> LocalExecutor {
        LocalExecutor { threads }
    }
}

impl ChunkExecutor for LocalExecutor {
    fn map_chunks<'w>(
        &self,
        chunks: &[Chunk],
        make_worker: &WorkerFactory<'w>,
        cancel: &CancelFlag,
    ) -> Vec<ChunkResult> {
        let run = |chunks: &[Chunk]| -> Vec<ChunkResult> {
            chunks
                .par_iter()
                .map_init(make_worker, |worker, chunk| {
                    run_one(&mut **worker, chunk, cancel)
                })
                .collect()
        };
        if self.threads == 0 {
            return run(chunks);
        }
        match rayon::ThreadPoolBuilder::new()
            .num_threads(self.threads)
            .build()
        {
            Ok(pool) => pool.install(|| run(chunks)),
            // A pool that will not build is not a reason to produce wrong numbers: fall back to
            // the global pool, which computes the same answer more slowly or less slowly.
            Err(_) => run(chunks),
        }
    }

    fn threads(&self) -> usize {
        if self.threads == 0 {
            rayon::current_num_threads()
        } else {
            self.threads
        }
    }

    fn name(&self) -> &'static str {
        "local"
    }
}

fn run_one(worker: &mut dyn ChunkWorker, chunk: &Chunk, cancel: &CancelFlag) -> ChunkResult {
    if cancel.is_cancelled() {
        return ChunkResult::Cancelled;
    }
    match worker.run(chunk) {
        Ok(out) => ChunkResult::Done(Box::new(out)),
        Err(e) => ChunkResult::Failed(e),
    }
}
