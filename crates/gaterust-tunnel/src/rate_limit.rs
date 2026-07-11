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
        let now = Instant::now();
        let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
        let nanos =
            (u128::from(bytes) * 1_000_000_000).div_ceil(u128::from(inner.bytes_per_second.get()));
        let cost = Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX));
        let delay = {
            let mut next = inner
                .next
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let earliest = next.checked_sub(inner.burst).unwrap_or(now);
            let scheduled = now.max(earliest);
            *next = scheduled + cost;
            scheduled.saturating_duration_since(now)
        };
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unlimited_does_not_wait() {
        let limiter = RateLimiter::new(None);
        let started = Instant::now();
        limiter.acquire(1_000_000).await;
        assert!(started.elapsed() < Duration::from_millis(10));
    }
}
