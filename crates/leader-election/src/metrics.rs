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

//! Optional metrics trait for leader election observability.

/// Metrics hooks for leader election events.
pub trait LeaderMetrics: Send + Sync {
    /// Called when leadership is acquired.
    fn on_acquire(&self);
    /// Called when leadership is released or lost.
    fn on_release(&self);
    /// Called on a successful lease renewal.
    fn on_renew_success(&self);
    /// Called on a failed lease renewal attempt.
    fn on_renew_failure(&self);
}
