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

//! Leader callbacks trait.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// Callbacks invoked during leader election lifecycle events.
#[async_trait]
pub trait LeaderCallbacks: Send + Sync {
    /// Called when this instance becomes the leader.
    /// Receives a cancellation token that is cancelled when leadership is lost.
    async fn on_started_leading(&self, cancel: CancellationToken);

    /// Called when this instance stops being the leader (always called, even if never led).
    async fn on_stopped_leading(&self);

    /// Called when a new leader is observed (fire-and-forget, runs in a separate task).
    async fn on_new_leader(&self, identity: String);
}
