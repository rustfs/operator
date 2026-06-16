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
    vec![CaseSpec::new(
        Suite::Faults,
        "fault_io_eio_preserves_committed_objects",
        "Inject IOChaos EIO into one RustFS data volume and verify committed S3 objects remain readable with matching hashes after recovery.",
        "rustfs-workload/fault-injection",
        "faults",
    )]
}

#[cfg(test)]
mod tests {
    use super::cases;

    #[test]
    fn fault_case_inventory_matches_executable_tests() {
        let names = cases()
            .into_iter()
            .map(|case| case.name)
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["fault_io_eio_preserves_committed_objects"]);
    }
}
