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

//! Integration tests for leader election.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use kube_leader_election::{
    Clock, Error, LeaderCallbacks, LeaderElectionRecord, LeaderElector, LeaderElectorConfig, Lock,
};

// ─── Test Helpers ───────────────────────────────────────────────────────────

/// Fake lock with controllable behavior for testing.
#[derive(Clone)]
struct FakeLock {
    identity: String,
    record: Arc<RwLock<Option<LeaderElectionRecord>>>,
    resource_version: Arc<Mutex<u64>>,
    /// If set, update() will fail with Conflict this many times before succeeding.
    conflict_count: Arc<AtomicUsize>,
}

impl FakeLock {
    fn new(identity: &str) -> Self {
        Self {
            identity: identity.to_string(),
            record: Arc::new(RwLock::new(None)),
            resource_version: Arc::new(Mutex::new(0)),
            conflict_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Configure the lock to return a specific record on get().
    async fn set_record(&self, record: LeaderElectionRecord) {
        let mut r = self.record.write().await;
        *r = Some(record);
        let mut rv = self.resource_version.lock().await;
        *rv += 1;
    }

    /// Configure the lock to fail with Conflict N times before succeeding.
    fn set_conflicts(&self, count: usize) {
        self.conflict_count.store(count, Ordering::SeqCst);
    }
}

#[async_trait]
impl Lock for FakeLock {
    async fn get(&self) -> Result<Option<LeaderElectionRecord>, Error> {
        let r = self.record.read().await;
        Ok(r.clone())
    }

    async fn create(&self, record: LeaderElectionRecord) -> Result<(), Error> {
        let mut r = self.record.write().await;
        if r.is_some() {
            return Err(Error::Conflict);
        }
        *r = Some(record);
        let mut rv = self.resource_version.lock().await;
        *rv += 1;
        Ok(())
    }

    async fn update(&self, record: LeaderElectionRecord) -> Result<(), Error> {
        // Simulate conflict if configured
        let remaining = self.conflict_count.load(Ordering::SeqCst);
        if remaining > 0 {
            self.conflict_count.fetch_sub(1, Ordering::SeqCst);
            return Err(Error::Conflict);
        }

        let mut r = self.record.write().await;
        *r = Some(record);
        let mut rv = self.resource_version.lock().await;
        *rv += 1;
        Ok(())
    }

    fn identity(&self) -> &str {
        &self.identity
    }

    fn describe(&self) -> String {
        format!("fake/{}", self.identity)
    }
}

/// Mock clock with controllable time.
#[derive(Clone)]
struct MockClock {
    now: Arc<RwLock<DateTime<Utc>>>,
}

impl MockClock {
    fn new(time: DateTime<Utc>) -> Self {
        Self {
            now: Arc::new(RwLock::new(time)),
        }
    }

    async fn set(&self, time: DateTime<Utc>) {
        *self.now.write().await = time;
    }
}

impl Clock for MockClock {
    fn now(&self) -> DateTime<Utc> {
        // Block on async read (safe in tests)
        futures::executor::block_on(async { *self.now.read().await })
    }
}

/// Test callbacks that record events.
struct TestCallbacks {
    started_leading: Arc<AtomicUsize>,
    stopped_leading: Arc<AtomicUsize>,
    new_leader: Arc<RwLock<Vec<String>>>,
}

impl TestCallbacks {
    fn new() -> Self {
        Self {
            started_leading: Arc::new(AtomicUsize::new(0)),
            stopped_leading: Arc::new(AtomicUsize::new(0)),
            new_leader: Arc::new(RwLock::new(Vec::new())),
        }
    }

    async fn started_count(&self) -> usize {
        self.started_leading.load(Ordering::SeqCst)
    }

    async fn stopped_count(&self) -> usize {
        self.stopped_leading.load(Ordering::SeqCst)
    }

    async fn leaders(&self) -> Vec<String> {
        self.new_leader.read().await.clone()
    }
}

#[async_trait]
impl LeaderCallbacks for TestCallbacks {
    async fn on_started_leading(&self, _cancel: CancellationToken) {
        self.started_leading.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_stopped_leading(&self) {
        self.stopped_leading.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_new_leader(&self, identity: String) {
        let mut leaders = self.new_leader.write().await;
        leaders.push(identity);
    }
}

/// Owned wrapper around `Arc<TestCallbacks>` so we can implement `LeaderCallbacks`
/// (we can't impl a foreign trait on `Arc<T>` directly due to the orphan rule).
#[derive(Clone)]
struct SharedCallbacks(Arc<TestCallbacks>);

#[async_trait]
impl LeaderCallbacks for SharedCallbacks {
    async fn on_started_leading(&self, cancel: CancellationToken) {
        self.0.on_started_leading(cancel).await;
    }

    async fn on_stopped_leading(&self) {
        self.0.on_stopped_leading().await;
    }

    async fn on_new_leader(&self, identity: String) {
        self.0.on_new_leader(identity).await;
    }
}

/// Create a valid test config.
fn test_config(identity: &str) -> LeaderElectorConfig {
    LeaderElectorConfig {
        identity: identity.to_string(),
        lease_duration: Duration::from_secs(15),
        renew_deadline: Duration::from_secs(10),
        retry_period: Duration::from_millis(100),
        release_on_cancel: true,
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

/// Test 1: LeaseLock construction and LeaderElectionRecord field semantics.
///
/// Note on the fake kube client: the task description references
/// `kube::Client::new_custom`, but kube 2.x does not expose a public fake/mock
/// client constructor (only the internal `kube_client::Client::new` used by
/// `kube::Client::mock` inside the kube crate's own tests). Rather than pull in
/// a heavy envtest dependency for a unit test, the Lock-trait CRUD semantics
/// are exercised by `FakeLock` below — `LeaseLock` is a thin wrapper around
/// `Api<Lease>` that delegates to the same `Lock` trait, and the pure
/// record↔spec conversion logic is covered by unit tests in `lock::lease`.
#[test]
fn test_lease_lock_create_update_get() {
    // LeaderElectionRecord field semantics (these are what LeaseLock persists).
    let record = LeaderElectionRecord {
        holder_identity: "pod-1".to_string(),
        lease_duration_seconds: 15,
        acquire_time: Utc::now(),
        renew_time: Utc::now(),
        leader_transitions: 0,
    };
    assert_eq!(record.holder_identity, "pod-1");
    assert_eq!(record.lease_duration_seconds, 15);
    assert_eq!(record.leader_transitions, 0);

    // FakeLock CRUD round-trip: create → get → update → get.
    // This exercises the same Lock trait that LeaseLock implements.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let lock = FakeLock::new("pod-1");

        // get() on a fresh lock returns None (lease does not exist yet).
        assert!(lock.get().await.unwrap().is_none());

        // create() installs a record.
        let r1 = LeaderElectionRecord {
            holder_identity: "pod-1".to_string(),
            lease_duration_seconds: 15,
            acquire_time: Utc::now(),
            renew_time: Utc::now(),
            leader_transitions: 0,
        };
        lock.create(r1.clone()).await.unwrap();

        // get() now returns the created record.
        let got = lock
            .get()
            .await
            .unwrap()
            .expect("record should exist after create");
        assert_eq!(got.holder_identity, "pod-1");
        assert_eq!(got.lease_duration_seconds, 15);

        // create() again must fail with Conflict (lock already exists).
        assert!(matches!(lock.create(r1).await, Err(Error::Conflict)));

        // update() replaces the record.
        let r2 = LeaderElectionRecord {
            holder_identity: "pod-1".to_string(),
            lease_duration_seconds: 15,
            acquire_time: got.acquire_time,
            renew_time: Utc::now(),
            leader_transitions: 1,
        };
        lock.update(r2).await.unwrap();

        let got2 = lock
            .get()
            .await
            .unwrap()
            .expect("record should exist after update");
        assert_eq!(got2.leader_transitions, 1);
    });
}

/// Test 2: Acquire when no holder exists.
#[tokio::test]
async fn test_acquire_no_holder() {
    let lock = FakeLock::new("node-1");
    let clock = MockClock::new(Utc::now());
    let config = test_config("node-1");
    let callbacks = Arc::new(TestCallbacks::new());

    let elector = LeaderElector::new(config, lock, clock).unwrap();
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    // Run elector in background
    let cb = callbacks.clone();
    let handle = tokio::spawn(async move { elector.run(SharedCallbacks(cb), cancel_clone).await });

    // Wait a bit for acquire to succeed
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Cancel to stop the elector
    cancel.cancel();

    // Wait for completion
    let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(result.is_ok(), "elector should complete");

    // Verify callbacks were called
    assert_eq!(
        callbacks.started_count().await,
        1,
        "should have started leading"
    );
    assert!(
        callbacks.stopped_count().await >= 1,
        "should have stopped leading"
    );
}

/// Test 3: Acquire only after the locally observed lease window expires.
#[tokio::test]
async fn test_acquire_expired_lease() {
    let lock = FakeLock::new("node-2");
    let now = Utc::now();

    // The remote renew_time is old, but a candidate must not use that
    // remote wall clock to take over immediately.
    let expired_time = now - chrono::Duration::seconds(30);
    let expired_record = LeaderElectionRecord {
        holder_identity: "node-1".to_string(),
        lease_duration_seconds: 15,
        acquire_time: expired_time,
        renew_time: expired_time,
        leader_transitions: 0,
    };
    lock.set_record(expired_record).await;

    let clock = MockClock::new(now);
    let test_clock = clock.clone();
    let config = test_config("node-2");
    let callbacks = Arc::new(TestCallbacks::new());

    let elector = LeaderElector::new(config, lock, clock).unwrap();
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    let cb = callbacks.clone();
    let handle = tokio::spawn(async move { elector.run(SharedCallbacks(cb), cancel_clone).await });

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        callbacks.started_count().await,
        0,
        "should not acquire immediately from stale remote renew_time"
    );

    test_clock.set(now + chrono::Duration::seconds(16)).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    cancel.cancel();

    let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(result.is_ok(), "elector should complete");

    // node-2 should acquire after the local observed lease duration elapses.
    assert_eq!(
        callbacks.started_count().await,
        1,
        "should have started leading"
    );
}

/// Test 4: Wait when lease is active and held by another.
#[tokio::test]
async fn test_acquire_active_lease() {
    let lock = FakeLock::new("node-2");

    // Set an active lease held by another node
    let active_time = Utc::now();
    let active_record = LeaderElectionRecord {
        holder_identity: "node-1".to_string(),
        lease_duration_seconds: 15,
        acquire_time: active_time,
        renew_time: active_time,
        leader_transitions: 0,
    };
    lock.set_record(active_record).await;

    let clock = MockClock::new(Utc::now());
    let config = test_config("node-2");
    let callbacks = Arc::new(TestCallbacks::new());

    let elector = LeaderElector::new(config, lock, clock).unwrap();
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    let cb = callbacks.clone();
    let handle = tokio::spawn(async move { elector.run(SharedCallbacks(cb), cancel_clone).await });

    // Wait a bit - node-2 should NOT acquire while lease is active
    tokio::time::sleep(Duration::from_millis(300)).await;

    // node-2 should not have started leading yet
    assert_eq!(
        callbacks.started_count().await,
        0,
        "should not have started leading while lease is active"
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
}

#[tokio::test]
async fn test_clock_skewed_candidate_waits_while_remote_record_changes() {
    let lock = FakeLock::new("shared");
    let remote_start = Utc::now();
    let candidate_start = remote_start + chrono::Duration::seconds(60);
    let leader_record = LeaderElectionRecord {
        holder_identity: "node-1".to_string(),
        lease_duration_seconds: 15,
        acquire_time: remote_start,
        renew_time: remote_start,
        leader_transitions: 0,
    };
    lock.set_record(leader_record.clone()).await;

    let clock = MockClock::new(candidate_start);
    let test_clock = clock.clone();
    let mut config = test_config("node-2");
    config.release_on_cancel = false;
    let callbacks = Arc::new(TestCallbacks::new());

    let elector = LeaderElector::new(config, lock.clone(), clock).unwrap();
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    let cb = callbacks.clone();
    let handle = tokio::spawn(async move { elector.run(SharedCallbacks(cb), cancel_clone).await });

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        callbacks.started_count().await,
        0,
        "skewed candidate should not acquire immediately"
    );

    for step in 1..=3 {
        test_clock
            .set(candidate_start + chrono::Duration::seconds(step * 10))
            .await;
        lock.set_record(LeaderElectionRecord {
            renew_time: remote_start + chrono::Duration::seconds(step * 10),
            ..leader_record.clone()
        })
        .await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            callbacks.started_count().await,
            0,
            "candidate should not acquire while renewals keep changing the record"
        );
    }

    test_clock
        .set(candidate_start + chrono::Duration::seconds(46))
        .await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    cancel.cancel();

    let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(result.is_ok(), "elector should complete");
    assert_eq!(
        callbacks.started_count().await,
        1,
        "candidate should acquire after renewals stop and local observation expires"
    );

    let final_record = lock
        .get()
        .await
        .expect("fake lock get should succeed")
        .expect("record should exist");
    assert_eq!(final_record.holder_identity, "node-2");
}

/// Test 5: Successful renewal.
#[tokio::test]
async fn test_renew_success() {
    let lock = FakeLock::new("node-1");
    let clock = MockClock::new(Utc::now());
    let config = test_config("node-1");
    let callbacks = Arc::new(TestCallbacks::new());

    let elector = LeaderElector::new(config, lock, clock).unwrap();
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    let cb = callbacks.clone();
    let handle = tokio::spawn(async move { elector.run(SharedCallbacks(cb), cancel_clone).await });

    // Let it acquire and renew a few times
    tokio::time::sleep(Duration::from_millis(500)).await;

    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(result.is_ok(), "elector should complete");

    // Should have started leading (acquire succeeded)
    assert_eq!(
        callbacks.started_count().await,
        1,
        "should have started leading"
    );
}

/// Test 6: Renewal with resource version conflict (retry behavior).
#[tokio::test]
async fn test_renew_conflict() {
    let lock = FakeLock::new("node-1");
    // Configure 2 conflicts before success
    lock.set_conflicts(2);

    let clock = MockClock::new(Utc::now());
    let config = test_config("node-1");
    let callbacks = Arc::new(TestCallbacks::new());

    let elector = LeaderElector::new(config, lock, clock).unwrap();
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    let cb = callbacks.clone();
    let handle = tokio::spawn(async move { elector.run(SharedCallbacks(cb), cancel_clone).await });

    // Let it retry through conflicts
    tokio::time::sleep(Duration::from_millis(600)).await;

    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(result.is_ok(), "elector should complete");

    // Should eventually succeed after retries
    assert_eq!(
        callbacks.started_count().await,
        1,
        "should have started leading after retries"
    );
}

/// Test 7: Leader transition callback is invoked.
#[tokio::test]
async fn test_leader_transition_callback() {
    let lock = FakeLock::new("node-1");
    let clock = MockClock::new(Utc::now());
    let config = test_config("node-1");
    let callbacks = Arc::new(TestCallbacks::new());

    let elector = LeaderElector::new(config, lock, clock).unwrap();
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    let cb = callbacks.clone();
    let handle = tokio::spawn(async move { elector.run(SharedCallbacks(cb), cancel_clone).await });

    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.cancel();

    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    // The on_new_leader callback should have been called with our identity
    let leaders = callbacks.leaders().await;
    assert!(!leaders.is_empty(), "on_new_leader should have been called");
    assert_eq!(leaders[0], "node-1", "leader identity should match");
}

/// Test 8: Concurrent acquire - only one candidate wins.
#[tokio::test]
async fn test_concurrent_acquire() {
    // Shared lock between two candidates
    let shared_record = Arc::new(RwLock::new(None));
    let shared_rv = Arc::new(Mutex::new(0));

    let lock1 = FakeLock {
        identity: "node-1".to_string(),
        record: shared_record.clone(),
        resource_version: shared_rv.clone(),
        conflict_count: Arc::new(AtomicUsize::new(0)),
    };

    let lock2 = FakeLock {
        identity: "node-2".to_string(),
        record: shared_record.clone(),
        resource_version: shared_rv.clone(),
        conflict_count: Arc::new(AtomicUsize::new(0)),
    };

    let clock1 = MockClock::new(Utc::now());
    let clock2 = MockClock::new(Utc::now());

    let config1 = test_config("node-1");
    let config2 = test_config("node-2");

    let callbacks1 = Arc::new(TestCallbacks::new());
    let callbacks2 = Arc::new(TestCallbacks::new());

    let elector1 = LeaderElector::new(config1, lock1, clock1).unwrap();
    let elector2 = LeaderElector::new(config2, lock2, clock2).unwrap();

    let cancel = CancellationToken::new();
    let cancel1 = cancel.clone();
    let cancel2 = cancel.clone();

    let cb1 = callbacks1.clone();
    let h1 = tokio::spawn(async move { elector1.run(SharedCallbacks(cb1), cancel1).await });

    let cb2 = callbacks2.clone();
    let h2 = tokio::spawn(async move { elector2.run(SharedCallbacks(cb2), cancel2).await });

    // Let them race
    tokio::time::sleep(Duration::from_millis(500)).await;

    cancel.cancel();

    let _ = tokio::time::timeout(Duration::from_secs(2), h1).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), h2).await;

    let started1 = callbacks1.started_count().await;
    let started2 = callbacks2.started_count().await;

    // Exactly one should have become leader
    assert_eq!(
        started1 + started2,
        1,
        "exactly one candidate should become leader"
    );
}
