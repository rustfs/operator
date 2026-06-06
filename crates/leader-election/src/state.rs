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

//! Leader state observation (watch-based handle for consumers).

use tokio::sync::watch;

/// Current state of the leader elector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaderState {
    /// Not yet participating in election.
    Pending,
    /// This instance is the current leader.
    Leading,
    /// Following another leader identified by the given identity.
    Following(String),
    /// Elector has failed with the given error message.
    Failed(String),
}

/// Handle for observing leader election state from outside the elector task.
#[derive(Debug)]
pub struct LeaderElectorHandle {
    pub(crate) state_rx: watch::Receiver<LeaderState>,
}

impl LeaderElectorHandle {
    /// Returns true if this instance is currently the leader (non-blocking).
    pub fn is_leader(&self) -> bool {
        *self.state_rx.borrow() == LeaderState::Leading
    }

    /// Returns the current leader identity, if known.
    pub fn current_leader(&self) -> Option<String> {
        match &*self.state_rx.borrow() {
            LeaderState::Leading => None, // we are the leader
            LeaderState::Following(id) => Some(id.clone()),
            _ => None,
        }
    }

    /// Subscribe to state changes as an async stream.
    pub fn state_stream(&self) -> impl futures::Stream<Item = LeaderState> + '_ {
        let mut rx = self.state_rx.clone();
        async_stream::stream! {
            loop {
                if rx.changed().await.is_err() {
                    break;
                }
                yield rx.borrow().clone();
            }
        }
    }
}
