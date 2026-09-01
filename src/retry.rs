use std::time::{Duration, SystemTime};

const MAX_RETRY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_JITTER_BASIS_POINTS: u16 = 5_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryDecision {
    pub retryable: bool,
    pub retry_after: Option<Duration>,
}

impl RetryDecision {
    pub const fn permanent() -> Self {
        Self {
            retryable: false,
            retry_after: None,
        }
    }

    pub fn retryable(retry_after: Option<Duration>) -> Self {
        Self {
            retryable: true,
            retry_after: retry_after.map(|duration| duration.min(MAX_RETRY_AFTER)),
        }
    }
}

/// Bounded retry schedule shared by provider adapters.
///
/// Attempt zero is the first retry after the initial delivery attempt. Jitter is
/// deterministic from caller-supplied entropy so tests can reproduce an exact
/// schedule and replicas do not need process-global randomness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    base_delay: Duration,
    max_delay: Duration,
    max_attempts: u32,
    jitter_basis_points: u16,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new(
            Duration::from_secs(1),
            Duration::from_secs(5 * 60),
            8,
            2_000,
        )
    }
}

impl RetryPolicy {
    pub fn new(
        base_delay: Duration,
        max_delay: Duration,
        max_attempts: u32,
        jitter_basis_points: u16,
    ) -> Self {
        let base_delay = base_delay.max(Duration::from_millis(1));
        let max_delay = max_delay.max(base_delay).min(MAX_RETRY_AFTER);
        Self {
            base_delay,
            max_delay,
            max_attempts: max_attempts.max(1),
            jitter_basis_points: jitter_basis_points.min(MAX_JITTER_BASIS_POINTS),
        }
    }

    pub const fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// Returns the delay for a retry attempt or `None` after the attempt budget
    /// is exhausted. Provider `Retry-After` is treated as a floor and remains
    /// capped at 24 hours even when an upstream sends an extreme value.
    pub fn delay_for_attempt(
        &self,
        attempt: u32,
        entropy: u64,
        retry_after: Option<Duration>,
    ) -> Option<Duration> {
        if attempt >= self.max_attempts {
            return None;
        }

        let exponent = attempt.min(63);
        let multiplier = 1_u128 << exponent;
        let uncapped_millis = self.base_delay.as_millis().saturating_mul(multiplier);
        let capped_millis = uncapped_millis.min(self.max_delay.as_millis());

        // Downward-only jitter keeps every local schedule at or below the
        // configured cap. The caller can derive entropy from a stable job ID so
        // retries spread across replicas while remaining reproducible.
        let jitter_window = capped_millis
            .saturating_mul(u128::from(self.jitter_basis_points))
            / 10_000;
        let jitter_offset = if jitter_window == 0 {
            0
        } else {
            u128::from(entropy) % (jitter_window + 1)
        };
        let jittered = duration_from_millis(capped_millis.saturating_sub(jitter_offset));
        let provider_floor = retry_after
            .map(|duration| duration.min(MAX_RETRY_AFTER))
            .unwrap_or_default();

        Some(jittered.max(provider_floor).min(MAX_RETRY_AFTER))
    }
}

fn duration_from_millis(millis: u128) -> Duration {
    Duration::from_millis(u64::try_from(millis).unwrap_or(u64::MAX))
}

/// Parse either a delta-seconds or HTTP-date `Retry-After` value.
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();

    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds).min(MAX_RETRY_AFTER));
    }

    let retry_at = httpdate::parse_http_date(value).ok()?;
    let duration = retry_at.duration_since(now).unwrap_or_default();
    Some(duration.min(MAX_RETRY_AFTER))
}

/// Provider-neutral baseline classification. Adapters may refine individual
/// provider error codes, but must not turn a permanent target failure into an
/// unbounded retry loop.
pub fn classify_http_status(status: u16, retry_after: Option<Duration>) -> RetryDecision {
    match status {
        408 | 425 | 429 | 500..=599 => RetryDecision::retryable(retry_after),
        _ => RetryDecision::permanent(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_delta_seconds_and_caps_extreme_values() {
        assert_eq!(
            parse_retry_after("120", SystemTime::UNIX_EPOCH),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            parse_retry_after("999999", SystemTime::UNIX_EPOCH),
            Some(MAX_RETRY_AFTER)
        );
    }

    #[test]
    fn parses_http_dates() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let value = httpdate::fmt_http_date(now + Duration::from_secs(45));
        assert_eq!(
            parse_retry_after(&value, now),
            Some(Duration::from_secs(45))
        );
    }

    #[test]
    fn classifies_transient_and_permanent_statuses() {
        assert!(classify_http_status(429, None).retryable);
        assert!(classify_http_status(503, Some(Duration::from_secs(3))).retryable);
        assert!(!classify_http_status(400, None).retryable);
        assert!(!classify_http_status(401, None).retryable);
        assert!(!classify_http_status(404, None).retryable);
    }

    #[test]
    fn retry_schedule_is_deterministic_exponential_and_bounded() {
        let policy = RetryPolicy::new(
            Duration::from_secs(1),
            Duration::from_secs(8),
            5,
            2_000,
        );

        assert_eq!(
            policy.delay_for_attempt(0, 0, None),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            policy.delay_for_attempt(1, 0, None),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            policy.delay_for_attempt(2, 0, None),
            Some(Duration::from_secs(4))
        );
        assert_eq!(
            policy.delay_for_attempt(3, 0, None),
            Some(Duration::from_secs(8))
        );
        assert_eq!(
            policy.delay_for_attempt(4, 0, None),
            Some(Duration::from_secs(8))
        );
        assert_eq!(policy.delay_for_attempt(5, 0, None), None);
    }

    #[test]
    fn jitter_never_exceeds_the_local_cap_or_drops_below_the_configured_window() {
        let policy = RetryPolicy::new(
            Duration::from_secs(10),
            Duration::from_secs(10),
            1,
            2_000,
        );

        for entropy in 0..10_000 {
            let delay = policy
                .delay_for_attempt(0, entropy, None)
                .expect("first retry is allowed");
            assert!(delay <= Duration::from_secs(10));
            assert!(delay >= Duration::from_secs(8));
        }
    }

    #[test]
    fn provider_retry_after_is_honored_as_a_bounded_floor() {
        let policy = RetryPolicy::new(
            Duration::from_secs(1),
            Duration::from_secs(30),
            2,
            2_000,
        );

        assert_eq!(
            policy.delay_for_attempt(0, 17, Some(Duration::from_secs(90))),
            Some(Duration::from_secs(90))
        );
        assert_eq!(
            policy.delay_for_attempt(0, 17, Some(Duration::from_secs(999_999))),
            Some(MAX_RETRY_AFTER)
        );
    }

    #[test]
    fn invalid_configuration_is_normalized_to_safe_bounds() {
        let policy = RetryPolicy::new(Duration::ZERO, Duration::ZERO, 0, u16::MAX);
        assert_eq!(policy.max_attempts(), 1);
        let delay = policy
            .delay_for_attempt(0, u64::MAX, None)
            .expect("normalized policy permits one retry");
        assert!(delay <= Duration::from_millis(1));
        assert!(delay >= Duration::from_micros(500));
        assert_eq!(policy.delay_for_attempt(1, 0, None), None);
    }
}
