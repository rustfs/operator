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

//! Pool boundary:
//!   - list/status and decommission lifecycle operations for tenant pools.

use super::helpers::build_query_pairs;
use super::{
    POOLS_CANCEL_PATH, POOLS_DECOMMISSION_PATH, POOLS_LIST_PATH, POOLS_STATUS_PATH,
    RustfsAdminClient, RustfsClientError, RustfsPoolListItem, RustfsPoolStatus,
};

impl RustfsAdminClient {
    // Pool duties: list/status and decommission lifecycle operations.

    pub async fn list_pools(&self) -> Result<Vec<RustfsPoolListItem>, RustfsClientError> {
        let body = self
            .send_admin_request("GET", POOLS_LIST_PATH, "", "", None)
            .await?;

        serde_json::from_str::<Vec<RustfsPoolListItem>>(&body)
            .map_err(|_| RustfsClientError::ParseResponseFailed)
    }

    pub async fn pool_status_by_id(
        &self,
        pool_id: &str,
    ) -> Result<RustfsPoolStatus, RustfsClientError> {
        let query = build_query_pairs(&[("by-id", "true"), ("pool", pool_id)]);
        let body = self
            .send_admin_request("GET", POOLS_STATUS_PATH, &query, "", None)
            .await?;

        serde_json::from_str::<RustfsPoolStatus>(&body)
            .map_err(|_| RustfsClientError::ParseResponseFailed)
    }

    pub async fn start_pool_decommission_by_id(
        &self,
        pool_id: &str,
    ) -> Result<(), RustfsClientError> {
        let query = build_query_pairs(&[("by-id", "true"), ("pool", pool_id)]);
        self.send_admin_request("POST", POOLS_DECOMMISSION_PATH, &query, "", None)
            .await?;
        Ok(())
    }

    pub async fn cancel_pool_decommission_by_id(
        &self,
        pool_id: &str,
    ) -> Result<(), RustfsClientError> {
        let query = build_query_pairs(&[("by-id", "true"), ("pool", pool_id)]);
        self.send_admin_request("POST", POOLS_CANCEL_PATH, &query, "", None)
            .await?;
        Ok(())
    }
}
