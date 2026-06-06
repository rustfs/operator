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

//! Leader election configuration and validation.

use std::time::Duration;

/// Configuration for the leader elector.
///
/// Constraint: `lease_duration > renew_deadline > retry_period * 1.2`
#[derive(Debug, Clone)]
pub struct LeaderElectorConfig {
    /// Unique identity for this instance (typically pod name or hostname).
    pub identity: String,
    /// Duration a non-leader waits before attempting to acquire (default 15s).
    pub lease_duration: Duration,
    /// Deadline within which the leader must successfully renew (default 10s).
    pub renew_deadline: Duration,
    /// Interval between retry attempts (default 2s).
    pub retry_period: Duration,
    /// Whether to release the lock when the cancel token fires (default true).
    pub release_on_cancel: bool,
}
