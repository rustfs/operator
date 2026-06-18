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

use anyhow::{Result, ensure};
use std::time::Duration;

use crate::framework::fault_config::FaultTestConfig;

pub const IO_EIO_SCENARIO: &str = "io-eio";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultScenario {
    pub name: String,
    pub duration: Duration,
    pub percent: u8,
    pub object_count: usize,
}

impl FaultScenario {
    pub fn from_config(config: &FaultTestConfig) -> Result<Self> {
        ensure!(
            config.scenario == IO_EIO_SCENARIO,
            "unsupported fault scenario {:?}; first implementation supports only {IO_EIO_SCENARIO:?}",
            config.scenario
        );
        ensure!(
            (1..=100).contains(&config.percent),
            "RUSTFS_FAULT_TEST_PERCENT must be in 1..=100, got {}",
            config.percent
        );
        ensure!(
            config.duration > Duration::ZERO,
            "RUSTFS_FAULT_TEST_DURATION_SECONDS must be greater than zero"
        );
        ensure!(
            config.workload_objects >= 4,
            "RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS must be at least 4"
        );

        Ok(Self {
            name: config.scenario.clone(),
            duration: config.duration,
            percent: config.percent,
            object_count: config.workload_objects,
        })
    }

    pub fn prefill_count(&self) -> usize {
        self.object_count / 2
    }

    pub fn mixed_workload_count(&self) -> usize {
        self.object_count - self.prefill_count()
    }
}

#[cfg(test)]
mod tests {
    use super::{FaultScenario, IO_EIO_SCENARIO};
    use crate::framework::fault_config::FaultTestConfig;
    use std::time::Duration;

    #[test]
    fn default_fault_scenario_is_io_eio_with_split_workload() {
        let config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        let scenario = FaultScenario::from_config(&config).expect("valid scenario");

        assert_eq!(scenario.name, IO_EIO_SCENARIO);
        assert_eq!(scenario.duration, Duration::from_secs(180));
        assert_eq!(scenario.percent, 20);
        assert_eq!(scenario.prefill_count(), 20);
        assert_eq!(scenario.mixed_workload_count(), 20);
    }

    #[test]
    fn unsupported_fault_scenario_is_rejected() {
        let mut config = FaultTestConfig::for_test("real-cluster", "fast-csi");
        config.scenario = "operator-restart".to_string();

        assert!(FaultScenario::from_config(&config).is_err());
    }
}
