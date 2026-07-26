use std::{
    num::NonZeroU64,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::time::Instant;

#[derive(Clone)]
pub(crate) struct RateLimiter {
    inner: Option<Arc<Limited>>,
}

struct Limited {
    bytes_per_second: NonZeroU64,
    next: Mutex<Instant>,
    burst: Duration,
}

impl RateLimiter {
    pub(crate) fn new(bytes_per_second: Option<NonZeroU64>) -> Self {
        let inner = bytes_per_second.map(|rate| {
            Arc::new(Limited {
                bytes_per_second: rate,
                next: Mutex::new(Instant::now()),
                burst: Duration::from_millis(100),
            })
        });
        Self { inner }
    }

    pub(crate) async fn acquire(&self, bytes: usize) {
        let Some(inner) = &self.inner else {
            return;
        };
        let delay = inner.reserve(bytes, Instant::now());
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
}

impl Limited {
    fn reserve(&self, bytes: usize, now: Instant) -> Duration {
        let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
        let nanos =
            (u128::from(bytes) * 1_000_000_000).div_ceil(u128::from(self.bytes_per_second.get()));
        let cost = Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX));
        let mut next = self
            .next
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *next = (*next).max(now) + cost;
        next.checked_sub(self.burst)
            .unwrap_or(now)
            .saturating_duration_since(now)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;

    use super::*;

    #[tokio::test]
    async fn unlimited_does_not_wait() {
        let limiter = RateLimiter::new(None);
        let started = Instant::now();
        limiter.acquire(1_000_000).await;
        assert!(started.elapsed() < Duration::from_millis(10));
    }

    #[test]
    fn limited_reservations_consume_burst_once() {
        let now = Instant::now();
        let limited = Limited {
            bytes_per_second: NonZeroU64::new(1_000).expect("测试速率非零"),
            next: Mutex::new(now),
            burst: Duration::from_millis(100),
        };

        assert_eq!(limited.reserve(50, now), Duration::ZERO);
        assert_eq!(limited.reserve(50, now), Duration::ZERO);
        assert_eq!(limited.reserve(50, now), Duration::from_millis(50));
        assert_eq!(limited.reserve(50, now), Duration::from_millis(100));
    }

    #[test]
    fn concurrent_reservations_share_one_budget() {
        let now = Instant::now();
        let limited = Arc::new(Limited {
            bytes_per_second: NonZeroU64::new(1_000).expect("测试速率非零"),
            next: Mutex::new(now),
            burst: Duration::from_millis(100),
        });
        let barrier = Arc::new(Barrier::new(4));
        let mut threads = Vec::new();
        for _ in 0..3 {
            let limited = Arc::clone(&limited);
            let barrier = Arc::clone(&barrier);
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                limited.reserve(100, now)
            }));
        }
        barrier.wait();
        let mut delays = threads
            .into_iter()
            .map(|thread| thread.join().expect("限速测试线程正常结束"))
            .collect::<Vec<_>>();
        delays.sort_unstable();

        assert_eq!(
            delays,
            [
                Duration::ZERO,
                Duration::from_millis(100),
                Duration::from_millis(200),
            ]
        );
    }
}
