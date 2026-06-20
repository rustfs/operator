# Copyright 2025 RustFS Team
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

SHELL := /bin/bash

FAULT_SCRIPT := $(CURDIR)/scripts/fault-test.sh
MANIFEST := $(CURDIR)/Cargo.toml

.PHONY: help fault-check fault-preflight fault-run fault-run-regular fault-run-dm fault-cleanup

help:
	@echo "RustFS e2e fault-test package"
	@echo ""
	@echo "Usage:"
	@echo "  make -C e2e fault-check"
	@echo "  make -C e2e fault-preflight [SCENARIO=io-eio]"
	@echo "  make -C e2e fault-run SCENARIO=io-eio"
	@echo "  make -C e2e fault-run-regular"
	@echo "  make -C e2e fault-run-dm"
	@echo "  make -C e2e fault-cleanup"
	@echo ""
	@echo "Required runtime environment:"
	@echo "  RUSTFS_FAULT_TEST_EXPECTED_CONTEXT"
	@echo "  RUSTFS_FAULT_TEST_STORAGE_CLASS"
	@echo "  RUSTFS_FAULT_TEST_SERVER_IMAGE"
	@echo ""
	@echo "See e2e/FAULT_TESTING.md for cluster preparation and safety requirements."

fault-check:
	bash -n $(FAULT_SCRIPT)
	cargo fmt --manifest-path $(MANIFEST) --all --check
	cargo test --manifest-path $(MANIFEST)
	cargo clippy --manifest-path $(MANIFEST) --all-targets -- -D warnings

fault-preflight:
	@bash $(FAULT_SCRIPT) preflight "$(or $(SCENARIO),io-eio)"

fault-run:
	@test -n "$(SCENARIO)" || (echo "SCENARIO is required" >&2; exit 2)
	@bash $(FAULT_SCRIPT) run "$(SCENARIO)"

fault-run-regular:
	@bash $(FAULT_SCRIPT) run-regular

fault-run-dm:
	@bash $(FAULT_SCRIPT) run dm-flakey

fault-cleanup:
	@bash $(FAULT_SCRIPT) cleanup
