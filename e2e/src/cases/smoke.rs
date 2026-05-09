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

use super::{CaseSpec, Suite};

pub fn cases() -> Vec<CaseSpec> {
    vec![
        CaseSpec::new(
            Suite::Smoke,
            "smoke_kind_install",
            "Create a dedicated Kind cluster, load images, and install operator, console, and console-web.",
            "kind/install",
            "smoke",
        ),
        CaseSpec::new(
            Suite::Smoke,
            "smoke_console_health_openapi",
            "Verify /healthz, /readyz, and OpenAPI document availability before deeper API tests.",
            "console/http",
            "smoke",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::cases;

    #[test]
    fn smoke_case_inventory_is_not_empty() {
        assert!(cases().len() >= 2);
    }
}
