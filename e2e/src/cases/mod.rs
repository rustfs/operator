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

pub mod console;
pub mod faults;
pub mod operator;
pub mod smoke;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Suite {
    Smoke,
    Operator,
    Console,
    Faults,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaseSpec {
    pub suite: Suite,
    pub name: &'static str,
    pub description: &'static str,
    pub boundary: &'static str,
    pub ci_phase: &'static str,
}

impl CaseSpec {
    pub const fn new(
        suite: Suite,
        name: &'static str,
        description: &'static str,
        boundary: &'static str,
        ci_phase: &'static str,
    ) -> Self {
        Self {
            suite,
            name,
            description,
            boundary,
            ci_phase,
        }
    }
}

pub fn all_cases() -> Vec<CaseSpec> {
    let mut cases = Vec::new();
    cases.extend(smoke::cases());
    cases.extend(operator::cases());
    cases.extend(console::cases());
    cases.extend(faults::cases());
    cases
}

#[cfg(test)]
mod tests {
    use super::{Suite, all_cases};
    use std::collections::{HashMap, HashSet};

    #[test]
    fn release_plan_has_clear_suite_boundaries() {
        let cases = all_cases();
        let suites = cases.iter().map(|case| case.suite).collect::<HashSet<_>>();

        assert!(suites.contains(&Suite::Smoke));
        assert!(suites.contains(&Suite::Operator));
        assert!(suites.contains(&Suite::Console));
        assert!(suites.contains(&Suite::Faults));
    }

    #[test]
    fn case_names_are_unique() {
        let mut seen = HashSet::new();
        for case in all_cases() {
            assert!(
                seen.insert(case.name),
                "duplicate e2e case name: {}",
                case.name
            );
        }
    }

    #[test]
    fn cases_are_mapped_to_ci_phases_and_architecture_boundaries() {
        let missing = all_cases()
            .into_iter()
            .filter(|case| case.boundary.is_empty() || case.ci_phase.is_empty())
            .map(|case| case.name)
            .collect::<Vec<_>>();

        assert!(
            missing.is_empty(),
            "cases missing boundary/ci phase: {missing:?}"
        );
    }

    #[test]
    fn operator_and_console_cases_are_both_present() {
        let counts = all_cases()
            .into_iter()
            .fold(HashMap::new(), |mut acc, case| {
                *acc.entry(case.suite).or_insert(0usize) += 1;
                acc
            });

        assert!(counts.get(&Suite::Operator).copied().unwrap_or_default() >= 3);
        assert!(counts.get(&Suite::Console).copied().unwrap_or_default() >= 3);
    }
}
