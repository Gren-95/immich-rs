//! A fixed-size pool of std threads that processes items with retries, like the original
//! CLI's `Queue` (fastq with `retry: 3`, meaning at most three attempts per item).

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

/// Attempts per task before it is reported as failed.
pub const ATTEMPTS: u32 = 3;

/// Runs `worker` over `items` on up to `concurrency` threads, retrying failures up to
/// `attempts` times in total. Results are returned in the order of `items`.
pub fn run<T, R, F>(items: &[T], concurrency: usize, attempts: u32, worker: F) -> Vec<Result<R, String>>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> Result<R, String> + Sync,
{
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<Result<R, String>>>> = items.iter().map(|_| Mutex::new(None)).collect();
    let threads = concurrency.max(1).min(items.len());

    thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    let Some(item) = items.get(index) else { break };
                    let outcome = run_with_retries(item, attempts, &worker);
                    if let Ok(mut slot) = slots[index].lock() {
                        *slot = Some(outcome);
                    }
                }
            });
        }
    });

    slots
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .ok()
                .flatten()
                .unwrap_or_else(|| Err("Error: task did not run".to_string()))
        })
        .collect()
}

/// Calls `worker` until it succeeds or `attempts` calls have failed.
pub fn run_with_retries<T, R>(item: &T, attempts: u32, worker: &impl Fn(&T) -> Result<R, String>) -> Result<R, String> {
    let mut outcome = worker(item);
    let mut tries = 1;
    while outcome.is_err() && tries < attempts {
        tries += 1;
        outcome = worker(item);
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_input_order_and_retries_failures() {
        let calls = AtomicUsize::new(0);
        let items: Vec<u32> = (0..50).collect();
        let results = run(&items, 4, ATTEMPTS, |item| {
            calls.fetch_add(1, Ordering::SeqCst);
            if item % 10 == 0 {
                Err(format!("Error: {item}"))
            } else {
                Ok(item * 2)
            }
        });
        assert_eq!(results.len(), 50);
        assert_eq!(results[3], Ok(6));
        assert_eq!(results[10], Err("Error: 10".to_string()));
        // 45 successes on the first try, 5 failures tried three times each.
        assert_eq!(calls.load(Ordering::SeqCst), 45 + 5 * 3);
    }

    #[test]
    fn handles_empty_input() {
        let results: Vec<Result<u32, String>> = run(&Vec::<u32>::new(), 8, ATTEMPTS, |item| Ok(*item));
        assert!(results.is_empty());
    }
}
