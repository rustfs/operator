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

//! Leader election record — maps to Kubernetes Lease spec fields.

use chrono::{DateTime, Utc};

/// Record stored in the Lease object, representing the current election state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderElectionRecord {
    /// Identity of the current leader (empty if released).
    pub holder_identity: String,
    /// Lease duration in seconds (i32 to match K8s API int32).
    pub lease_duration_seconds: i32,
    /// Time when the current leader acquired leadership.
    pub acquire_time: DateTime<Utc>,
    /// Time of the most recent renewal.
    pub renew_time: DateTime<Utc>,
    /// Number of leader transitions observed.
    pub leader_transitions: i32,
}
