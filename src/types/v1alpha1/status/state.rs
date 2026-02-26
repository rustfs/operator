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
    #[strum(to_string = "Initialized")]
    Initialized,

    #[strum(to_string = "Statefulset not controlled by operator")]
    NotOwned,

    #[strum(to_string = "Pool Decommissioning Not Allowed")]
    DecommissioningNotAllowed,

    #[strum(to_string = "Provisioning IO Service")]
    ProvisioningIOService,

    #[strum(to_string = "Provisioning Console Service")]
    ProvisioningConsoleService,

    #[strum(to_string = "Provisioning Headless Service")]
    ProvisioningHeadlessService,

    #[strum(to_string = "Multiple tenants exist in the namespace")]
    MultipleTenantsExist,
}
