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

//! Locally observed election state (mirrors client-go observedRecord).

use chrono::{DateTime, Utc};

use crate::record::LeaderElectionRecord;

/// State observed locally from the most recent successful Get or Update.
#[derive(Debug, Clone, Default)]
pub struct ObservedState {
    /// Most recently observed election record.
    pub record: Option<LeaderElectionRecord>,
    /// Local time when the record was observed (used for lease validity).
    pub observed_time: Option<DateTime<Utc>>,
    /// Leader identity already reported via on_new_leader callback (dedup).
    pub reported_leader: Option<String>,
}
