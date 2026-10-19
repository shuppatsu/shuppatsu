//! Work split over all cores with scoped threads (no dependency): pure per-item work whose
//! results are collected in order, so the output never depends on the scheduling.

use std::sync::atomic::{AtomicUsize, Ordering};

/// `f` over `items` on all cores, results in the order of `items`.
pub fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    par_map_min(items, 1, f)
}

/// [`par_map`] with at least `min` items per thread's share (cheap items: fewer threads).
pub fn par_map_min<T: Sync, R: Send>(
    items: &[T],
    min: usize,
    f: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = items.len().div_ceil(threads * 4).max(min).max(1);
    if threads == 1 || items.len() <= chunk {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let chunks = items.len().div_ceil(chunk);
    let mut done: Vec<(usize, Vec<R>)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads.min(chunks))
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let c = next.fetch_add(1, Ordering::Relaxed);
                        if c >= chunks {
                            return out;
                        }
                        let range = c * chunk..((c + 1) * chunk).min(items.len());
                        out.push((c, items[range].iter().map(&f).collect()));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().expect("worker thread"))
            .collect()
    });
    done.sort_by_key(|(c, _)| *c);
    done.into_iter().flat_map(|(_, r)| r).collect()
}

/// [`par_map_min`] over `0..n`.
pub fn par_range<R: Send>(n: usize, min: usize, f: impl Fn(usize) -> R + Sync) -> Vec<R> {
    let idx: Vec<usize> = (0..n).collect();
    par_map_min(&idx, min, |&i| f(i))
}
