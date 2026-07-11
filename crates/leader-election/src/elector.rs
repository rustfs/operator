// Copyright 2024 RustFS Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Core leader elector logic.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::callbacks::LeaderCallbacks;
use crate::clock::Clock;
use crate::config::LeaderElectorConfig;
use crate::error::Error;
use crate::lock::Lock;
use crate::observed::ObservedState;
use crate::record::LeaderElectionRecord;
use crate::state::{LeaderElectorHandle, LeaderState};

/// The leader elector — drives the acquire/renew/release loop against a Lock backend.
pub struct LeaderElector<L: Lock> {
    config: LeaderElectorConfig,
    lock: L,
    observed: Arc<RwLock<ObservedState>>,
    clock: Box<dyn Clock>,
}

impl<L: Lock> LeaderElector<L> {
    /// Create a new LeaderElector with the given configuration, lock, and clock.
    ///
    /// Returns `Error::InvalidConfig` if the timing constraints are violated:
    /// `lease_duration > renew_deadline > retry_period * 1.2`
    pub fn new(
        config: LeaderElectorConfig,
        lock: L,
        clock: impl Clock + 'static,
    ) -> Result<Self, Error> {
        // Validate config constraints: lease_duration > renew_deadline > retry_period * 1.2
        let min_renew = config.retry_period.mul_f64(1.2);
        if config.lease_duration <= config.renew_deadline {
            return Err(Error::InvalidConfig {
                message: format!(
                    "lease_duration ({:?}) must be greater than renew_deadline ({:?})",
                    config.lease_duration, config.renew_deadline
                ),
            });
        }
        if config.renew_deadline <= min_renew {
            return Err(Error::InvalidConfig {
                message: format!(
                    "renew_deadline ({:?}) must be greater than retry_period * 1.2 ({:?})",
                    config.renew_deadline, min_renew
                ),
            });
        }
        if config.identity.is_empty() {
            return Err(Error::InvalidConfig {
                message: "identity must not be empty".to_string(),
            });
        }

        Ok(Self {
            config,
            lock,
            observed: Arc::new(RwLock::new(ObservedState::default())),
            clock: Box::new(clock),
        })
    }

    /// Run the leader election loop until the cancel token is triggered.
    ///
    /// Blocks the current task, cycling through acquire → renew → release phases.
    /// On transient lease/renewal loss, it retries acquisition after releasing.
    /// Always calls `on_stopped_leading` before returning, even if never became leader.
    pub async fn run(
        &self,
        callbacks: impl LeaderCallbacks + 'static,
        cancel: CancellationToken,
    ) -> Result<(), Error> {
        let callbacks = Arc::new(callbacks);
        info!(identity = %self.config.identity, lock = %self.lock.describe(), "starting leader election");

        loop {
            // Phase 1: acquire
            if cancel.is_cancelled() {
                callbacks.on_stopped_leading().await;
                return Ok(());
            }

            if !self.acquire(&cancel, &callbacks).await {
                // Cancelled during acquire
                callbacks.on_stopped_leading().await;
                return Ok(());
            }

            // We are now the leader. Create a child token for the leading task.
            let leading_cancel = CancellationToken::new();

            // Phase 2: start the user's leading function in a separate task
            let leading_handle = {
                let lc = leading_cancel.clone();
                let cb = callbacks.clone();
                tokio::spawn(async move {
                    cb.on_started_leading(lc).await;
                })
            };

            // Phase 3: renew loop (exits on cancel or renew failure)
            // false => stop; true => retry acquisition.
            let should_retry = self.renew(&cancel).await;

            // We lost leadership (or lost renew loop due cancel).
            // Stop the leading task.
            leading_cancel.cancel();
            // Wait for the leading task to finish.
            let _ = leading_handle.await;

            // Phase 4: release if configured
            if self.config.release_on_cancel {
                self.release().await;
            }

            if !should_retry {
                info!(identity = %self.config.identity, "stopped leading");
                // Phase 5: notify stopped
                callbacks.on_stopped_leading().await;
                return Ok(());
            }

            warn!(identity = %self.config.identity, "lost leadership; retrying election");
        }
    }

    /// Spawn the elector as a background task, returning a handle for state observation
    /// and a JoinHandle for error propagation.
    pub fn spawn(
        self,
        callbacks: impl LeaderCallbacks + 'static,
        cancel: CancellationToken,
    ) -> (
        LeaderElectorHandle,
        tokio::task::JoinHandle<Result<(), Error>>,
    )
    where
        L: 'static,
    {
        let (state_tx, state_rx) = tokio::sync::watch::channel(LeaderState::Pending);
        let handle = LeaderElectorHandle { state_rx };
        let join = tokio::spawn(async move {
            // Wrap callbacks to also update the watch channel
            let wrapped = StateTrackingCallbacks {
                inner: callbacks,
                state_tx,
            };
            self.run(wrapped, cancel).await
        });
        (handle, join)
    }

    // ─── Core algorithm ───────────────────────────────────────────────

    /// Acquire phase: loop trying to acquire the lock until success or cancellation.
    async fn acquire(
        &self,
        cancel: &CancellationToken,
        callbacks: &Arc<impl LeaderCallbacks + ?Sized>,
    ) -> bool {
        info!(identity = %self.config.identity, "attempting to acquire leader lock");
        loop {
            // Jittered sleep, interruptible by cancel
            let jittered = self.jittered_retry_period();
            tokio::select! {
                _ = tokio::time::sleep(jittered) => {},
                _ = cancel.cancelled() => {
                    debug!(identity = %self.config.identity, "acquire cancelled");
                    return false;
                }
            }

            if self.try_acquire_or_renew().await {
                info!(identity = %self.config.identity, "successfully acquired lease");
                self.maybe_report_transition(callbacks).await;
                return true;
            }
        }
    }

    /// Renew phase: keep renewing the lease, giving up after renew_deadline of failures.
    /// Returns `true` when leadership should be re-acquired, `false` when stopping.
    async fn renew(&self, cancel: &CancellationToken) -> bool {
        loop {
            if cancel.is_cancelled() {
                debug!(identity = %self.config.identity, "renew cancelled");
                return false;
            }

            if self.poll_renew(cancel).await {
                // Renewal succeeded at least once; wait before next cycle, interruptible.
                tokio::select! {
                    _ = tokio::time::sleep(self.config.retry_period) => {},
                    _ = cancel.cancelled() => {
                        debug!(identity = %self.config.identity, "renew sleep cancelled");
                        return false;
                    }
                }
            } else {
                // Failed to renew within renew_deadline — give up leadership.
                if cancel.is_cancelled() {
                    debug!(identity = %self.config.identity, "renew cancelled");
                    return false;
                }

                warn!(identity = %self.config.identity, "failed to renew lease within deadline, retrying election");
                return true;
            }
        }
    }

    /// Inner renew loop: retry within renew_deadline window.
    /// Returns `true` if at least one renewal succeeded.
    /// Returns `false` if the renew window expires or cancellation is requested.
    async fn poll_renew(&self, cancel: &CancellationToken) -> bool {
        let deadline = self.clock.now()
            + chrono::Duration::from_std(self.config.renew_deadline)
                .unwrap_or(chrono::Duration::seconds(10));
        loop {
            if cancel.is_cancelled() {
                return false;
            }

            if self.try_acquire_or_renew().await {
                return true;
            }
            if self.clock.now() >= deadline {
                return false;
            }
            tokio::select! {
                _ = tokio::time::sleep(self.config.retry_period) => {}
                _ = cancel.cancelled() => {
                    return false;
                }
            }
        }
    }

    /// Try to acquire or renew the lease in a single attempt.
    ///
    /// Fast path: if we are already leader and lease is still valid, just update.
    /// Slow path: get current state, decide if we can take over, then create/update.
    async fn try_acquire_or_renew(&self) -> bool {
        let now = self.clock.now();

        // Fast path: we are leader and lease is still valid — just renew.
        if self.is_leader().await && self.is_lease_valid(now).await {
            let record = self.build_record(now).await;
            match self.lock.update(record.clone()).await {
                Ok(()) => {
                    self.set_observed_record(record, now).await;
                    return true;
                }
                Err(e) => {
                    debug!(error = %e, "fast-path update failed, falling through to slow path");
                    // Fall through to slow path
                }
            }
        }

        // Slow path: Get current record
        let old_record = match self.lock.get().await {
            Ok(Some(r)) => {
                self.observe_record(r.clone(), now).await;
                r
            }
            Ok(None) => {
                // No existing lease — create one
                let record = self.build_record(now).await;
                return match self.lock.create(record.clone()).await {
                    Ok(()) => {
                        debug!(identity = %self.config.identity, "created new lease");
                        self.set_observed_record(record, now).await;
                        true
                    }
                    Err(e) => {
                        debug!(error = %e, "failed to create lease");
                        false
                    }
                };
            }
            Err(e) => {
                debug!(error = %e, "failed to get lease");
                return false;
            }
        };

        // Check if we can take over
        if !old_record.holder_identity.is_empty()
            && self.is_observed_lease_valid(&old_record, now).await
            && !self.is_leader_with(&old_record)
        {
            // Lease is held by someone else and still valid — give up this attempt
            return false;
        }

        // Build new record
        let mut record = self.build_record(now).await;
        if !self.is_leader_with(&old_record) {
            // New leader taking over — increment transitions
            record.leader_transitions = old_record.leader_transitions + 1;
        } else {
            // We were already leader — preserve acquire time and transition count
            record.acquire_time = old_record.acquire_time;
            record.leader_transitions = old_record.leader_transitions;
        }

        match self.lock.update(record.clone()).await {
            Ok(()) => {
                self.set_observed_record(record, now).await;
                true
            }
            Err(e) => {
                debug!(error = %e, "failed to update lease");
                false
            }
        }
    }

    /// Release the lease by clearing the holder identity.
    async fn release(&self) -> bool {
        let old_record = match self.lock.get().await {
            Ok(Some(r)) => r,
            Ok(None) => return true,
            Err(e) => {
                warn!(error = %e, "failed to get lease for release");
                return false;
            }
        };

        if !self.is_leader_with(&old_record) {
            return true;
        }

        let now = self.clock.now();
        let record = LeaderElectionRecord {
            holder_identity: String::new(),
            lease_duration_seconds: 1,
            acquire_time: now,
            renew_time: now,
            leader_transitions: old_record.leader_transitions,
        };

        match self.lock.update(record).await {
            Ok(()) => {
                info!(identity = %self.config.identity, "released leader lock");
                true
            }
            Err(e) => {
                warn!(error = %e, "failed to release leader lock");
                false
            }
        }
    }

    // ─── Helpers ──────────────────────────────────────────────────────

    /// Check if we are the current leader based on observed state.
    async fn is_leader(&self) -> bool {
        let observed = self.observed.read().await;
        observed
            .record
            .as_ref()
            .map(|r| r.holder_identity == self.config.identity)
            .unwrap_or(false)
    }

    /// Check if we are the leader according to a specific record.
    fn is_leader_with(&self, record: &LeaderElectionRecord) -> bool {
        record.holder_identity == self.config.identity
    }

    /// Check if the observed lease is still valid (based on observed_time + lease_duration).
    async fn is_lease_valid(&self, now: DateTime<Utc>) -> bool {
        let observed = self.observed.read().await;
        match (observed.record.as_ref(), observed.observed_time) {
            (Some(record), Some(obs_time)) => {
                let duration = Duration::from_secs(record.lease_duration_seconds as u64);
                now < obs_time + duration
            }
            _ => false,
        }
    }

    /// Check if a specific record's lease is still valid based on local observation time.
    async fn is_observed_lease_valid(
        &self,
        record: &LeaderElectionRecord,
        now: DateTime<Utc>,
    ) -> bool {
        let observed = self.observed.read().await;
        if observed.record.as_ref() != Some(record) {
            return false;
        }

        match observed.observed_time {
            Some(observed_time) => {
                let duration = Duration::from_secs(record.lease_duration_seconds as u64);
                now < observed_time + duration
            }
            None => false,
        }
    }

    /// Build a new election record for this identity.
    async fn build_record(&self, now: DateTime<Utc>) -> LeaderElectionRecord {
        let observed = self.observed.read().await;
        let acquire_time = if self.is_leader_inner(&observed) {
            // Preserve existing acquire time if we're already leader
            observed
                .record
                .as_ref()
                .map(|r| r.acquire_time)
                .unwrap_or(now)
        } else {
            now
        };

        LeaderElectionRecord {
            holder_identity: self.config.identity.clone(),
            lease_duration_seconds: self.config.lease_duration.as_secs() as i32,
            acquire_time,
            renew_time: now,
            leader_transitions: observed
                .record
                .as_ref()
                .map(|r| r.leader_transitions)
                .unwrap_or(0),
        }
    }

    /// Internal leader check using an already-borrowed observed state.
    fn is_leader_inner(&self, observed: &ObservedState) -> bool {
        observed
            .record
            .as_ref()
            .map(|r| r.holder_identity == self.config.identity)
            .unwrap_or(false)
    }

    /// Observe a record fetched from the lock backend.
    async fn observe_record(&self, record: LeaderElectionRecord, now: DateTime<Utc>) {
        let mut observed = self.observed.write().await;
        if observed.record.as_ref() != Some(&record) {
            observed.record = Some(record);
            observed.observed_time = Some(now);
        } else if observed.observed_time.is_none() {
            observed.observed_time = Some(now);
        }
    }

    /// Record a successful local create/update acknowledged by the lock backend.
    async fn set_observed_record(&self, record: LeaderElectionRecord, now: DateTime<Utc>) {
        let mut observed = self.observed.write().await;
        observed.record = Some(record);
        observed.observed_time = Some(now);
    }

    /// Check if a new leader has been observed and fire the on_new_leader callback.
    /// Deduplicates: only fires when the leader identity changes.
    async fn maybe_report_transition(&self, callbacks: &Arc<impl LeaderCallbacks + ?Sized>) {
        let mut observed = self.observed.write().await;
        let current_leader = observed
            .record
            .as_ref()
            .map(|r| r.holder_identity.clone())
            .unwrap_or_default();

        // Dedup: skip if we already reported this leader
        if observed.reported_leader.as_deref() == Some(&current_leader) {
            return;
        }

        observed.reported_leader = Some(current_leader.clone());

        if !current_leader.is_empty() {
            debug!(new_leader = %current_leader, "observed new leader");
            // Fire the on_new_leader callback (runs in caller's task; callbacks are expected
            // to be fast or spawn their own work).
            callbacks.on_new_leader(current_leader).await;
        }
    }

    /// Add jitter to retry period (factor 1.2, matching client-go).
    fn jittered_retry_period(&self) -> Duration {
        let jitter = self.config.retry_period.as_secs_f64() * 0.2 * rand::random::<f64>();
        self.config.retry_period + Duration::from_secs_f64(jitter)
    }
}

/// Internal callbacks wrapper that updates the watch channel on state transitions.
struct StateTrackingCallbacks<C: LeaderCallbacks> {
    inner: C,
    state_tx: tokio::sync::watch::Sender<LeaderState>,
}

#[async_trait::async_trait]
impl<C: LeaderCallbacks> LeaderCallbacks for StateTrackingCallbacks<C> {
    async fn on_started_leading(&self, cancel: CancellationToken) {
        let _ = self.state_tx.send(LeaderState::Leading);
        self.inner.on_started_leading(cancel).await;
    }

    async fn on_stopped_leading(&self) {
        let _ = self.state_tx.send(LeaderState::Pending);
        self.inner.on_stopped_leading().await;
    }

    async fn on_new_leader(&self, identity: String) {
        // Update state channel: if we're not currently Leading, report Following
        let is_leading = matches!(&*self.state_tx.borrow(), LeaderState::Leading);
        if !is_leading && !identity.is_empty() {
            let _ = self.state_tx.send(LeaderState::Following(identity.clone()));
        }
        self.inner.on_new_leader(identity).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::SystemClock;

    /// Minimal config for tests (passes validation).
    fn test_config(identity: &str) -> LeaderElectorConfig {
        LeaderElectorConfig {
            identity: identity.to_string(),
            lease_duration: Duration::from_secs(15),
            renew_deadline: Duration::from_secs(10),
            retry_period: Duration::from_secs(2),
            release_on_cancel: true,
        }
    }

    #[test]
    fn test_config_validation_ok() {
        let config = test_config("node-1");
        let result = LeaderElector::new(config, DummyLock::new("node-1"), SystemClock);
        assert!(result.is_ok());
    }

    #[test]
    fn test_config_validation_lease_too_short() {
        let config = LeaderElectorConfig {
            identity: "node-1".to_string(),
            lease_duration: Duration::from_secs(5),
            renew_deadline: Duration::from_secs(10),
            retry_period: Duration::from_secs(2),
            release_on_cancel: true,
        };
        let result = LeaderElector::new(config, DummyLock::new("node-1"), SystemClock);
        assert!(matches!(result, Err(Error::InvalidConfig { .. })));
    }

    #[test]
    fn test_config_validation_renew_too_short() {
        let config = LeaderElectorConfig {
            identity: "node-1".to_string(),
            lease_duration: Duration::from_secs(15),
            renew_deadline: Duration::from_secs(2),
            retry_period: Duration::from_secs(2),
            release_on_cancel: true,
        };
        let result = LeaderElector::new(config, DummyLock::new("node-1"), SystemClock);
        assert!(matches!(result, Err(Error::InvalidConfig { .. })));
    }

    #[test]
    fn test_config_validation_empty_identity() {
        let config = LeaderElectorConfig {
            identity: String::new(),
            lease_duration: Duration::from_secs(15),
            renew_deadline: Duration::from_secs(10),
            retry_period: Duration::from_secs(2),
            release_on_cancel: true,
        };
        let result = LeaderElector::new(config, DummyLock::new(""), SystemClock);
        assert!(matches!(result, Err(Error::InvalidConfig { .. })));
    }

    #[test]
    fn test_jittered_retry_period() {
        let config = test_config("node-1");
        let elector = LeaderElector::new(config, DummyLock::new("node-1"), SystemClock).unwrap();
        for _ in 0..100 {
            let jittered = elector.jittered_retry_period();
            // Should be between retry_period and retry_period * 1.2
            assert!(jittered >= Duration::from_secs(2));
            assert!(jittered <= Duration::from_secs_f64(2.4 + 0.001));
        }
    }

    #[test]
    fn test_is_leader_with() {
        let config = test_config("node-1");
        let elector = LeaderElector::new(config, DummyLock::new("node-1"), SystemClock).unwrap();

        let record = LeaderElectionRecord {
            holder_identity: "node-1".to_string(),
            lease_duration_seconds: 15,
            acquire_time: Utc::now(),
            renew_time: Utc::now(),
            leader_transitions: 0,
        };
        assert!(elector.is_leader_with(&record));

        let record_other = LeaderElectionRecord {
            holder_identity: "node-2".to_string(),
            ..record
        };
        assert!(!elector.is_leader_with(&record_other));
    }

    #[tokio::test]
    async fn test_observed_lease_valid_uses_local_observed_time() {
        let config = test_config("node-1");
        let elector = LeaderElector::new(config, DummyLock::new("node-1"), SystemClock).unwrap();
        let now = Utc::now();

        let record = LeaderElectionRecord {
            holder_identity: "node-2".to_string(),
            lease_duration_seconds: 15,
            acquire_time: now - chrono::Duration::seconds(30),
            renew_time: now - chrono::Duration::seconds(30),
            leader_transitions: 0,
        };
        elector.observe_record(record.clone(), now).await;

        assert!(elector.is_observed_lease_valid(&record, now).await);
        assert!(
            elector
                .is_observed_lease_valid(&record, now + chrono::Duration::seconds(14))
                .await
        );
        assert!(
            !elector
                .is_observed_lease_valid(&record, now + chrono::Duration::seconds(16))
                .await
        );
    }

    #[tokio::test]
    async fn test_same_observed_record_does_not_refresh_observed_time() {
        let config = test_config("node-1");
        let elector = LeaderElector::new(config, DummyLock::new("node-1"), SystemClock).unwrap();
        let now = Utc::now();

        let record = LeaderElectionRecord {
            holder_identity: "node-2".to_string(),
            lease_duration_seconds: 15,
            acquire_time: now,
            renew_time: now,
            leader_transitions: 0,
        };
        elector.observe_record(record.clone(), now).await;
        elector
            .observe_record(record.clone(), now + chrono::Duration::seconds(10))
            .await;

        assert!(
            !elector
                .is_observed_lease_valid(&record, now + chrono::Duration::seconds(16))
                .await
        );
    }

    // ─── Dummy lock for unit tests ────────────────────────────────────

    struct DummyLock {
        identity: String,
    }

    impl DummyLock {
        fn new(identity: &str) -> Self {
            Self {
                identity: identity.to_string(),
            }
        }
    }

    #[async_trait::async_trait]
    impl Lock for DummyLock {
        async fn get(&self) -> Result<Option<LeaderElectionRecord>, Error> {
            Ok(None)
        }
        async fn create(&self, _record: LeaderElectionRecord) -> Result<(), Error> {
            Ok(())
        }
        async fn update(&self, _record: LeaderElectionRecord) -> Result<(), Error> {
            Ok(())
        }
        fn identity(&self) -> &str {
            &self.identity
        }
        fn describe(&self) -> String {
            format!("dummy/{}", self.identity)
        }
    }
}
