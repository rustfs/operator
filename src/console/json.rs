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

use axum::{
    Json, async_trait,
    extract::{FromRequest, Request},
};
use serde::de::DeserializeOwned;

use crate::console::error::Error;

/// JSON request extractor that preserves the Console API error envelope.
#[derive(Debug)]
pub struct ConsoleJson<T>(pub T);

#[async_trait]
impl<S, T> FromRequest<S> for ConsoleJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = Error;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::<T>::from_request(request, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(Error::from_json_rejection)
    }
}
