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

//! RustFS COSI v1alpha1 driver library (Identity + Provisioner gRPC services).

pub mod backend;
pub mod driver;
pub mod parameters;
pub mod policy;

pub mod proto {
    pub mod cosi {
        pub mod v1alpha1 {
            tonic::include_proto!("cosi.v1alpha1");
        }
    }
}

pub use driver::{DRIVER_NAME, IdentityService, ProvisionerService};
