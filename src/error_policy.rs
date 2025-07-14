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

use crate::context::Context;
use crate::error::Error;
use crate::types::v1alpha1::tenant::Tenant;
use kube::runtime::controller::Action;
use std::sync::Arc;
use std::time::Duration;

pub fn error_policy(_object: Arc<Tenant>, error: &Error, _ctx: Arc<Context>) -> Action {
    if error.is_not_found() {
        Action::await_change()
    } else {
        Action::requeue(Duration::from_secs(5))
    }
}
