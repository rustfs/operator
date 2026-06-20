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
.PHONY: console-lint console-build console-fmt console-fmt-check
.PHONY: e2e-check e2e-live-create .e2e-live-install-cert-manager e2e-live-run e2e-live-update e2e-live-delete

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
	@echo "  make docker-build-operator  - Build the unified operator + console image (IMAGE_REPO?=rustfs/operator IMAGE_TAG?=dev)"
	@echo "  make docker-build-console-web - Build the legacy split console-web image (CONSOLE_WEB_IMAGE_REPO?=rustfs/console-web CONSOLE_WEB_IMAGE_TAG?=dev)"
	@echo "  make docker-build-all       - Build both unified operator and legacy console-web images"
	@echo "  make console-lint     - Run frontend ESLint checks (console-web)"
	@echo "  make console-build    - Build frontend static assets (console-web)"
	@echo "  make console-fmt     - Format frontend code with Prettier (console-web)"
	@echo "  make console-fmt-check - Check frontend formatting with Prettier (console-web)"
	@echo "  make e2e-check        - Check Rust-native e2e harness (fmt + test + clippy)"
	@echo "  make e2e-live-create  - Clean dedicated storage, recreate live Kind environment, install cert-manager, and load e2e image"
	@echo "  make e2e-live-run     - Run all non-destructive live suites in the existing live environment"
	@echo "  make e2e-live-update  - Rebuild image and update the live environment (load + rollout)"
	@echo "  make e2e-live-delete  - Delete live Kind environment and clean dedicated storage"

# pre-commit checks: Rust main crate + e2e harness + frontend (lint + build + format checks)
pre-commit: fmt-check clippy test e2e-check console-lint console-build console-fmt-check
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

# Run frontend ESLint checks. Run pnpm install in console-web first.
console-lint:
	cd console-web && pnpm run lint

# Build frontend. Run pnpm install in console-web first.
console-build:
	cd console-web && pnpm run build

# Format frontend code with Prettier. Run pnpm install in console-web first.
console-fmt:
	cd console-web && pnpm run format

# Check frontend formatting with Prettier without modifying files.
console-fmt-check:
	cd console-web && pnpm run format:check

# Build the project.
build:
	cargo build --release

# Rust-native e2e harness (live-first, dedicated Kind)
E2E_MANIFEST ?= e2e/Cargo.toml
E2E_BIN ?= cargo run --manifest-path $(E2E_MANIFEST) --bin rustfs-e2e --
E2E_TEST_THREADS ?= 1
E2E_KUBE_CONTEXT ?= kind-rustfs-e2e
CERT_MANAGER_VERSION ?= v1.16.2
CERT_MANAGER_MANIFEST_URL ?= https://github.com/cert-manager/cert-manager/releases/download/$(CERT_MANAGER_VERSION)/cert-manager.yaml
CERT_MANAGER_ROLLOUT_TIMEOUT ?= 180s
E2E_LIVE_ENV ?= RUSTFS_E2E_LIVE=1 RUSTFS_E2E_CERT_MANAGER_VERSION=$(CERT_MANAGER_VERSION)

# Rust-native e2e harness checks (non-live; ignored live tests remain opt-in)
e2e-check:
	cargo fmt --manifest-path $(E2E_MANIFEST) --all --check
	cargo test --manifest-path $(E2E_MANIFEST)
	cargo clippy --manifest-path $(E2E_MANIFEST) --all-targets -- -D warnings

# 4-command live workflow. Keep helper steps inline so the public Make surface stays small.
e2e-live-create:
	docker build --network host -t rustfs/operator:e2e .
	docker build --network host -t rustfs/console-web:e2e -f console-web/Dockerfile console-web
	$(E2E_LIVE_ENV) $(E2E_BIN) kind-delete || true
	$(E2E_LIVE_ENV) $(E2E_BIN) kind-create
	$(E2E_LIVE_ENV) $(E2E_BIN) kind-load-images
	$(MAKE) .e2e-live-install-cert-manager

.e2e-live-install-cert-manager:
	kubectl --context $(E2E_KUBE_CONTEXT) apply -f $(CERT_MANAGER_MANIFEST_URL)
	kubectl --context $(E2E_KUBE_CONTEXT) -n cert-manager rollout status deployment/cert-manager --timeout=$(CERT_MANAGER_ROLLOUT_TIMEOUT)
	kubectl --context $(E2E_KUBE_CONTEXT) -n cert-manager rollout status deployment/cert-manager-cainjector --timeout=$(CERT_MANAGER_ROLLOUT_TIMEOUT)
	kubectl --context $(E2E_KUBE_CONTEXT) -n cert-manager rollout status deployment/cert-manager-webhook --timeout=$(CERT_MANAGER_ROLLOUT_TIMEOUT)

e2e-live-run:
	$(E2E_LIVE_ENV) $(E2E_BIN) assert-context
	$(MAKE) .e2e-live-install-cert-manager
	$(E2E_LIVE_ENV) $(E2E_BIN) deploy-dev
	$(E2E_LIVE_ENV) $(E2E_BIN) reset-live-fixtures
	RUSTFS_E2E_LIVE=1 cargo test --manifest-path $(E2E_MANIFEST) --test smoke -- --ignored --test-threads=$(E2E_TEST_THREADS) --nocapture
	RUSTFS_E2E_LIVE=1 cargo test --manifest-path $(E2E_MANIFEST) --test operator -- --ignored --test-threads=$(E2E_TEST_THREADS) --nocapture
	RUSTFS_E2E_LIVE=1 cargo test --manifest-path $(E2E_MANIFEST) --test sts_functional -- --ignored --test-threads=$(E2E_TEST_THREADS) --nocapture
	RUSTFS_E2E_LIVE=1 cargo test --manifest-path $(E2E_MANIFEST) --test console -- --ignored --test-threads=$(E2E_TEST_THREADS) --nocapture
	RUSTFS_E2E_LIVE=1 cargo test --manifest-path $(E2E_MANIFEST) --test cert_manager_tls -- --ignored --test-threads=$(E2E_TEST_THREADS) --nocapture
	@echo "configured live e2e suites passed."

e2e-live-update:
	docker build --network host -t rustfs/operator:e2e .
	docker build --network host -t rustfs/console-web:e2e -f console-web/Dockerfile console-web
	$(E2E_LIVE_ENV) $(E2E_BIN) assert-context
	$(E2E_LIVE_ENV) $(E2E_BIN) kind-load-images
	$(E2E_LIVE_ENV) $(E2E_BIN) rollout-dev

e2e-live-delete:
	$(E2E_LIVE_ENV) $(E2E_BIN) kind-delete


# Build Docker images. The operator image includes the controller, Console API,
# and embedded console-web static assets. The console-web image is retained for
# legacy split frontend deployments.
docker-build-operator:
	docker build -t $(IMAGE_REPO):$(IMAGE_TAG) .

docker-build-console-web:
	docker build -t $(CONSOLE_WEB_IMAGE_REPO):$(CONSOLE_WEB_IMAGE_TAG) -f console-web/Dockerfile console-web

docker-build-all: docker-build-operator docker-build-console-web
