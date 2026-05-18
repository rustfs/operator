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
            Suite::Sts,
            "sts_live_tokenreview_policybinding_and_assume_role_succeeds",
            "Create a TLS Tenant, projected ServiceAccount token, PolicyBinding, and RustFS canned policy, then verify HTTPS Operator STS returns temporary S3 credentials.",
            "sts/tls-tokenreview-policybinding-assumerole",
            "sts",
        ),
        CaseSpec::new(
            Suite::Sts,
            "sts_live_rejects_non_tls_tenant",
            "Verify HTTPS Operator STS rejects a non-TLS Tenant before issuing temporary credentials.",
            "sts/non-tls-tenant-rejected",
            "sts",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::cases;

    #[test]
    fn sts_case_inventory_matches_executable_tests() {
        let names = cases()
            .into_iter()
            .map(|case| case.name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "sts_live_tokenreview_policybinding_and_assume_role_succeeds",
                "sts_live_rejects_non_tls_tenant"
            ]
        );
    }
}
