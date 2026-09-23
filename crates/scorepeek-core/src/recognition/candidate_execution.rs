//! Bounded parallel scheduling for pure core catalog scoring.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::recognition::shared::{CandidateExecution, SongCandidateDomain};

pub struct ParallelCandidateExecution;

impl CandidateExecution for ParallelCandidateExecution {
    fn map<T: Send, F: Fn(&SongCandidateDomain) -> T + Sync>(
        songs: &[SongCandidateDomain],
        map: F,
    ) -> Vec<T> {
        let desired_workers = 4.min(songs.len().max(1));
        let permits = CatalogParallelPermits::acquire(desired_workers.saturating_sub(1));
        let workers = 1 + permits.count;
        if workers == 1 || songs.len() < workers * 2 {
            return songs.iter().map(map).collect();
        }
        let chunk_size = songs.len().div_ceil(workers);
        std::thread::scope(|scope| {
            let mut handles = songs[chunk_size..]
                .chunks(chunk_size)
                .map(|chunk| {
                    let map = &map;
                    scope.spawn(move || chunk.iter().map(map).collect::<Vec<_>>())
                })
                .collect::<Vec<_>>();
            let mut output = songs[..chunk_size].iter().map(&map).collect::<Vec<_>>();
            for handle in handles.drain(..) {
                output.extend(handle.join().expect("catalog scoring worker panicked"));
            }
            output
        })
    }
}

struct CatalogParallelPermits {
    count: usize,
}

static CATALOG_PARALLEL_ACTIVE: AtomicUsize = AtomicUsize::new(0);

impl CatalogParallelPermits {
    fn acquire(requested: usize) -> Self {
        static MAXIMUM: OnceLock<usize> = OnceLock::new();
        let maximum = *MAXIMUM.get_or_init(|| {
            (std::thread::available_parallelism().map_or(1, usize::from) / 4).max(1)
        });
        let mut count = 0;
        while count < requested {
            let acquired = CATALOG_PARALLEL_ACTIVE
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                    (active < maximum).then_some(active + 1)
                })
                .is_ok();
            if !acquired {
                break;
            }
            count += 1;
        }
        Self { count }
    }
}

impl Drop for CatalogParallelPermits {
    fn drop(&mut self) {
        CATALOG_PARALLEL_ACTIVE.fetch_sub(self.count, Ordering::AcqRel);
    }
}
