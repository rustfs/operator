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

//! Kube-agnostic RustFS admin/S3/STS client.
//!
//! This crate contains the wire-protocol logic (request signing, HTTP
//! dispatch, response parsing) needed to talk to a RustFS server's admin,
//! S3 and STS APIs. It has no dependency on `kube` or `Tenant` types;
//! kube/Tenant-aware wrappers live in the `operator` crate's
//! `sts::rustfs_client` module.

mod admin_ops;
mod client;
mod core_ops;
mod credentials;
mod helpers;
mod pool_ops;
mod s3_ops;
mod sanitize;
mod sts_ops;

pub use client::{
    CreateBucketResult, RustfsAdminClient, RustfsClientError, RustfsCredentials,
    RustfsErasureBackend, RustfsErasureSetInfo, RustfsPoolDecommissionInfo, RustfsPoolListItem,
    RustfsPoolStatus, RustfsServerInfo, RustfsServerUsage,
};
pub use credentials::StsAssumeRoleCredentials;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
