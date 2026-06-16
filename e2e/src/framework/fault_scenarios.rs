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

use crate::framework::config::E2eConfig;

pub const IO_EIO_SCENARIO: &str = "io-eio";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultScenario {
    pub name: String,
    pub duration: Duration,
    pub percent: u8,
    pub object_count: usize,
}

impl FaultScenario {
    pub fn from_config(config: &E2eConfig) -> Result<Self> {
        ensure!(
            config.fault_scenario == IO_EIO_SCENARIO,
            "unsupported fault scenario {:?}; first implementation supports only {IO_EIO_SCENARIO:?}",
            config.fault_scenario
        );
        ensure!(
            (1..=100).contains(&config.fault_percent),
            "RUSTFS_E2E_FAULT_PERCENT must be in 1..=100, got {}",
            config.fault_percent
        );
        ensure!(
            config.fault_duration > Duration::ZERO,
            "RUSTFS_E2E_FAULT_DURATION_SECONDS must be greater than zero"
        );
        ensure!(
            config.fault_workload_objects >= 4,
            "RUSTFS_E2E_WORKLOAD_OBJECTS must be at least 4"
        );

        Ok(Self {
            name: config.fault_scenario.clone(),
            duration: config.fault_duration,
            percent: config.fault_percent,
            object_count: config.fault_workload_objects,
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
    use crate::framework::config::E2eConfig;
    use std::time::Duration;

    #[test]
    fn default_fault_scenario_is_io_eio_with_split_workload() {
        let scenario = FaultScenario::from_config(&E2eConfig::defaults()).expect("valid scenario");

        assert_eq!(scenario.name, IO_EIO_SCENARIO);
        assert_eq!(scenario.duration, Duration::from_secs(180));
        assert_eq!(scenario.percent, 20);
        assert_eq!(scenario.prefill_count(), 20);
        assert_eq!(scenario.mixed_workload_count(), 20);
    }

    #[test]
    fn unsupported_fault_scenario_is_rejected() {
        let mut config = E2eConfig::defaults();
        config.fault_scenario = "operator-restart".to_string();

        assert!(FaultScenario::from_config(&config).is_err());
    }
}
