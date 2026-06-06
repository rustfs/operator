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

//! Lock trait abstraction for leader election backends.

pub mod lease;

use async_trait::async_trait;

use crate::error::Error;
use crate::record::LeaderElectionRecord;

/// Abstraction over a distributed lock backend (e.g., Kubernetes Lease).
#[async_trait]
pub trait Lock: Send + Sync {
    /// Fetch the current election record. Returns None if the lock does not exist yet.
    async fn get(&self) -> Result<Option<LeaderElectionRecord>, Error>;

    /// Create a new lock with the given record (first-time election).
    async fn create(&self, record: LeaderElectionRecord) -> Result<(), Error>;

    /// Update an existing lock (relies on resourceVersion for optimistic concurrency).
    async fn update(&self, record: LeaderElectionRecord) -> Result<(), Error>;

    /// Returns the unique identity of this lock holder.
    fn identity(&self) -> &str;

    /// Returns a human-readable description of this lock (for logging/debugging).
    fn describe(&self) -> String;
}
