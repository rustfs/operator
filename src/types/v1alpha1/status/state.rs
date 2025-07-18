// Copyright 2025 RustFS Team
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

use strum::Display;

#[derive(Display)]
pub enum State {
    #[strum(serialize = "Initialized")]
    Initialized,

    #[strum(serialize = "Statefulset not controlled by operator")]
    NotOwned,

    #[strum(serialize = "Pool Decommissioning Not Allowed")]
    DecommissioningNotAllowed,

    #[strum(serialize = "Provisioning IO Service")]
    ProvisioningIOService,

    #[strum(serialize = "Provisioning Console Service")]
    ProvisioningConsoleService,

    #[strum(serialize = "Provisioning Headless Service")]
    ProvisioningHeadlessService,

    #[strum(serialize = "Multiple tenants exist in the namespace")]
    MultipleTenantsExist,
}
