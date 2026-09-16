use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// Process-local token bucket matching the proven `dd-email-sms-contact-rs`
/// behavior while also returning a bounded retry hint. Deployments that scale
/// horizontally must treat this as a per-pod safety cap and rely on provider
/// limits / shared infrastructure for an exact global quota.
#[derive(Debug)]
pub struct ContactRateLimiter {
    bucket: Mutex<TokenBucket>,
}

impl ContactRateLimiter {
    pub fn per_minute(limit: u32) -> Self {
        Self {
            bucket: Mutex::new(TokenBucket::new(limit.max(1) as f64)),
        }
    }

    pub async fn try_acquire(&self) -> Result<(), Duration> {
        self.bucket.lock().await.try_take()
    }
}

#[derive(Debug)]
struct TokenBucket {
    capacity: f64,
    tokens: f64,
    per_second: f64,
    last: Instant,
}

impl TokenBucket {
    fn new(per_minute: f64) -> Self {
        let capacity = per_minute.max(1.0);
        Self {
            capacity,
            tokens: capacity,
            per_second: per_minute / 60.0,
            last: Instant::now(),
        }
    }

    fn try_take(&mut self) -> Result<(), Duration> {
        let now = Instant::now();
        self.tokens = (self.tokens
            + now.duration_since(self.last).as_secs_f64() * self.per_second)
            .min(self.capacity);
        self.last = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            return Ok(());
        }

        let missing = (1.0 - self.tokens).max(0.0);
        let seconds = if self.per_second > 0.0 {
            missing / self.per_second
        } else {
            60.0
        };
        Err(Duration::from_secs_f64(seconds.clamp(0.05, 60.0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bucket_allows_burst_then_returns_retry_hint() {
        let limiter = ContactRateLimiter::per_minute(2);
        assert!(limiter.try_acquire().await.is_ok());
        assert!(limiter.try_acquire().await.is_ok());
        let retry = limiter.try_acquire().await.expect_err("bucket exhausted");
        assert!(retry > Duration::ZERO);
        assert!(retry <= Duration::from_secs(60));
    }
}
