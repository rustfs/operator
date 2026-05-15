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

.PHONY: pre-commit fmt fmt-check clippy test build help
.PHONY: docker-build-operator docker-build-console-web docker-build-all
.PHONY: console-lint console-fmt console-fmt-check
.PHONY: e2e-check e2e-live-create e2e-live-run e2e-live-update e2e-live-delete

# Default target
IMAGE_REPO ?= rustfs/operator
IMAGE_TAG  ?= dev
CONSOLE_WEB_IMAGE_REPO ?= rustfs/console-web
CONSOLE_WEB_IMAGE_TAG  ?= dev

help:
	@echo "RustFS Operator Makefile"
	@echo ""
	@echo "Usage:"
	@echo "  make pre-commit        - Run the full local gate (Rust + frontend), matching CI"
	@echo "  make fmt              - Format Rust code"
	@echo "  make fmt-check        - Check Rust formatting without modifying files"
	@echo "  make clippy           - Run clippy checks"
	@echo "  make test             - Run Rust tests"
	@echo "  make build            - Build the project"
	@echo "  make docker-build-operator  - Build the operator image (IMAGE_REPO?=rustfs/operator IMAGE_TAG?=dev)"
	@echo "  make docker-build-console-web - Build the console-web frontend image (CONSOLE_WEB_IMAGE_REPO?=rustfs/console-web CONSOLE_WEB_IMAGE_TAG?=dev)"
	@echo "  make docker-build-all       - Build both operator and console-web images"
	@echo "  make console-lint     - Run frontend ESLint checks (console-web)"
	@echo "  make console-fmt     - Format frontend code with Prettier (console-web)"
	@echo "  make console-fmt-check - Check frontend formatting with Prettier (console-web)"
	@echo "  make e2e-check        - Check Rust-native e2e harness (fmt + test + clippy)"
	@echo "  make e2e-live-create  - Clean dedicated storage, recreate live Kind environment and load e2e image"
	@echo "  make e2e-live-run     - Run all live suites (smoke/operator/console) in the existing live environment"
	@echo "  make e2e-live-update  - Rebuild image and update the live environment (load + rollout)"
	@echo "  make e2e-live-delete  - Delete live Kind environment and clean dedicated storage"

# pre-commit checks: Rust main crate + e2e harness + frontend (lint + format checks)
pre-commit: fmt-check clippy test e2e-check console-lint console-fmt-check
	@echo "pre-commit: all checks passed"

# Format Rust code.
fmt:
	cargo fmt --all

# Check Rust formatting without modifying files.
fmt-check:
	cargo fmt --all --check

# Run clippy checks.
clippy:
	cargo clippy --all-features -- -D warnings

# Run Rust tests.
test:
	cargo test --all

# Run frontend ESLint checks. Run npm install in console-web first.
console-lint:
	cd console-web && npm run lint

# Format frontend code with Prettier. Run npm install in console-web first.
console-fmt:
	cd console-web && npm run format

# Check frontend formatting with Prettier without modifying files.
console-fmt-check:
	cd console-web && npm run format:check

# Build the project.
build:
	cargo build --release

# Rust-native e2e harness (live-first, dedicated Kind)
E2E_MANIFEST ?= e2e/Cargo.toml
E2E_BIN ?= cargo run --manifest-path $(E2E_MANIFEST) --bin rustfs-e2e --
E2E_TEST_THREADS ?= 1

# Rust-native e2e harness checks (non-live; ignored live tests remain opt-in)
e2e-check:
	cargo fmt --manifest-path $(E2E_MANIFEST) --all --check
	cargo test --manifest-path $(E2E_MANIFEST)
	cargo clippy --manifest-path $(E2E_MANIFEST) --all-targets -- -D warnings

# 4-command live workflow. Keep helper steps inline so the public Make surface stays small.
e2e-live-create:
	docker build --network host -t rustfs/operator:e2e .
	docker build --network host -t rustfs/console-web:e2e -f console-web/Dockerfile console-web
	RUSTFS_E2E_LIVE=1 $(E2E_BIN) kind-delete || true
	RUSTFS_E2E_LIVE=1 $(E2E_BIN) kind-create
	RUSTFS_E2E_LIVE=1 $(E2E_BIN) kind-load-images

e2e-live-run:
	RUSTFS_E2E_LIVE=1 $(E2E_BIN) assert-context
	RUSTFS_E2E_LIVE=1 $(E2E_BIN) deploy-dev
	RUSTFS_E2E_LIVE=1 cargo test --manifest-path $(E2E_MANIFEST) --test smoke -- --ignored --test-threads=$(E2E_TEST_THREADS) --nocapture
	RUSTFS_E2E_LIVE=1 cargo test --manifest-path $(E2E_MANIFEST) --test operator -- --ignored --test-threads=$(E2E_TEST_THREADS) --nocapture
	RUSTFS_E2E_LIVE=1 cargo test --manifest-path $(E2E_MANIFEST) --test console -- --ignored --test-threads=$(E2E_TEST_THREADS) --nocapture
	@echo "configured live e2e suites passed."

e2e-live-update:
	docker build --network host -t rustfs/operator:e2e .
	docker build --network host -t rustfs/console-web:e2e -f console-web/Dockerfile console-web
	RUSTFS_E2E_LIVE=1 $(E2E_BIN) assert-context
	RUSTFS_E2E_LIVE=1 $(E2E_BIN) kind-load-images
	RUSTFS_E2E_LIVE=1 $(E2E_BIN) rollout-dev

e2e-live-delete:
	RUSTFS_E2E_LIVE=1 $(E2E_BIN) kind-delete


# Build Docker images. The operator image includes the controller and Console API;
# the console-web image contains the frontend static assets.
docker-build-operator:
	docker build -t $(IMAGE_REPO):$(IMAGE_TAG) .

docker-build-console-web:
	docker build -t $(CONSOLE_WEB_IMAGE_REPO):$(CONSOLE_WEB_IMAGE_TAG) -f console-web/Dockerfile console-web

docker-build-all: docker-build-operator docker-build-console-web
