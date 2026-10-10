//! Monotonic aggregate operation deadline (issue 740 P1).
//!
//! One deadline bounds an entire ensure/stop: the environment lock plus
//! every Docker command. Budgets are only ever consumed, never reset, so
//! a slow subprocess cannot push the aggregate past the configured limit.
//! A zero budget is reported before any work instead of being silently
//! converted into an unbounded wait.

use std::time::{Duration, Instant};

/// Monotonic deadline for one execution-environment operation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OperationDeadline {
    expires_at: Instant,
}

impl OperationDeadline {
    pub(crate) fn new(budget: Duration) -> Self {
        Self {
            expires_at: Instant::now() + budget,
        }
    }

    /// Remaining budget; errors once the deadline has passed so callers
    /// never convert an expired budget into an unbounded request.
    pub(crate) fn remaining(&self) -> anyhow::Result<Duration> {
        let remaining = self
            .expires_at
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        if remaining.is_zero() {
            anyhow::bail!("execution environment operation deadline exceeded");
        }
        Ok(remaining)
    }

    /// Remaining budget in whole milliseconds, never zero. A positive
    /// sub-millisecond remainder rounds up to one millisecond so a bounded
    /// call can never degrade into an unbounded or zero-length wait.
    pub(crate) fn remaining_ms(&self) -> anyhow::Result<u64> {
        Ok(self.remaining()?.as_millis().max(1) as u64)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::OperationDeadline;

    #[test]
    fn expired_deadline_errors_instead_of_unbounded() {
        let deadline = OperationDeadline::new(Duration::ZERO);
        assert!(deadline.remaining().is_err());
        assert!(deadline.remaining_ms().is_err());
    }

    #[test]
    fn positive_budget_reports_at_least_one_millisecond() {
        let deadline = OperationDeadline::new(Duration::from_millis(50));
        assert!(deadline.remaining_ms().expect("budget") >= 1);
    }
}
