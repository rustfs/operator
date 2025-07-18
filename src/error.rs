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

use std::borrow::Cow;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    KubeError(#[from] kube::Error),

    #[error("no namespace")]
    NoNamespace,

    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),

    #[error("{0}")]
    StrError(Cow<'static, str>),

    #[error("multiple initialized tenants")]
    MultiError,
}

impl Error {
    pub fn is_not_found(&self) -> bool {
        let Error::KubeError(kube::Error::Api(err)) = self else {
            return false;
        };
        err.reason == "NotFound" || err.code == 404
    }

    pub fn is_conflict(&self) -> bool {
        let Error::KubeError(kube::Error::Api(err)) = self else {
            return false;
        };
        err.reason == "Conflict" || err.code == 409
    }
}
