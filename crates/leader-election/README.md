# kube-leader-election

Kubernetes leader election for Rust operators, using Lease resources as the lock backend.

Semantics are aligned with [client-go leaderelection](https://github.com/kubernetes/client-go/tree/master/tools/leaderelection).

## Features

- **Lease-based locking** — uses `coordination.k8s.io/v1` Lease objects (K8s 1.14+)
- **Structured concurrency** — no implicit task spawning; caller controls lifecycle via `CancellationToken`
- **Lock trait abstraction** — pluggable backends, easy to test with fakes
- **Clock trait injection** — deterministic tests with `MockClock`
- **Observability** — `LeaderElectorHandle` with watch-channel state stream
- **Jittered retry** — randomized retry intervals to avoid thundering herd

## Quick Start

```rust
use kube_leader_election::{
    LeaderCallbacks, LeaderElector, LeaderElectorConfig, LeaseLock, SystemClock,
};
use kube::Client;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

let client = Client::try_default().await?;

// Create the lock (one per Lease name/namespace)
let lock = LeaseLock::new(client, "my-operator-lease", "default");

// Configure the elector
let config = LeaderElectorConfig {
    identity: "pod-abc123".into(),
    lease_duration: Duration::from_secs(15),
    renew_deadline: Duration::from_secs(10),
    retry_period: Duration::from_secs(2),
    release_on_cancel: true,
};

// Create the elector
let elector = LeaderElector::new(config, lock, SystemClock);

// Define callbacks
struct MyCallbacks;

#[async_trait::async_trait]
impl LeaderCallbacks for MyCallbacks {
    async fn on_started_leading(&self, cancel: CancellationToken) {
        // Run your controller loop here
        my_controller(cancel).await;
    }

    async fn on_stopped_leading(&self) {
        tracing::warn!("lost leadership, shutting down");
    }

    async fn on_new_leader(&self, identity: String) {
        tracing::info!("new leader: {}", identity);
    }
}

// Run — blocks until cancellation, retrying on transient leadership loss
let cancel = CancellationToken::new();
elector.run(MyCallbacks, cancel).await;
```

## Configuration

| Parameter | Default | Description |
|-----------|---------|-------------|
| `identity` | *(required)* | Unique ID for this instance (typically pod name) |
| `lease_duration` | 15s | Non-leaders wait this long before forcing takeover |
| `renew_deadline` | 10s | Leader retry window for renewing the lease |
| `retry_period` | 2s | Interval between acquire/renew attempts (with jitter) |
| `release_on_cancel` | `true` | Whether to release the lease when context is cancelled |

**Constraint:** `lease_duration > renew_deadline > retry_period × 1.2`

## RBAC

The operator needs the following permissions on the Lease resource:

```yaml
- apiGroups: ["coordination.k8s.io"]
  resources: ["leases"]
  verbs: ["get", "list", "watch", "create", "update", "patch", "delete"]
```

## Architecture

```
LeaderElector
├── acquire() — loop: try_acquire_or_renew → sleep(jittered retry_period)
├── renew()   — loop: try_acquire_or_renew within renew_deadline window
├── release() — clear holder_identity, preserve transition count
└── run()     — orchestrate acquire → on_started_leading → renew → on_stopped_leading (retries on lease loss)
```

The `Lock` trait abstracts the backend:

```rust
#[async_trait]
pub trait Lock: Send + Sync {
    async fn get(&self) -> Result<Option<LeaderElectionRecord>, Error>;
    async fn create(&self, record: LeaderElectionRecord) -> Result<(), Error>;
    async fn update(&self, record: LeaderElectionRecord) -> Result<(), Error>;
    fn identity(&self) -> &str;
    fn describe(&self) -> String;
}
```

`LeaseLock` is the built-in implementation backed by Kubernetes Lease objects.

## Testing

```bash
# Unit tests
cargo test -p kube-leader-election --lib

# Integration tests (uses FakeLock, no cluster needed)
cargo test -p kube-leader-election --test integration_tests
```

## Design Documentation

See [`docs/design.md`](docs/design.md) for the full technical design including:
- Client-go source analysis
- API design decisions
- Algorithm details (acquire/renew/release)
- Concurrency model

## License

Apache-2.0
