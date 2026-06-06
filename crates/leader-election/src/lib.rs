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

//! Kubernetes leader election for RustFS Operator.
//!
//! Provides active/standby mutual exclusion for multi-replica operator deployments
//! using Kubernetes Lease resources as the lock backend.

pub mod callbacks;
pub mod clock;
pub mod config;
pub mod elector;
pub mod error;
pub mod lock;
pub mod metrics;
pub mod observed;
pub mod record;
pub mod state;

// Re-export core public types for convenience.
pub use callbacks::LeaderCallbacks;
pub use clock::{Clock, SystemClock};
pub use config::LeaderElectorConfig;
pub use elector::LeaderElector;
pub use error::Error;
pub use lock::Lock;
pub use lock::lease::LeaseLock;
pub use record::LeaderElectionRecord;
pub use state::{LeaderElectorHandle, LeaderState};
