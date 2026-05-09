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

.PHONY: pre-commit fmt fmt-check clippy test build docker-build-operator docker-build-console-web docker-build-all help console-lint console-fmt console-fmt-check e2e-build-all e2e-build-operator-image e2e-build-console-web-image e2e-kind-create e2e-kind-delete e2e-kind-load-images e2e-assert-context e2e-deploy-dev e2e-storage-prepare e2e-smoke-live e2e-operator-live e2e-console-live e2e-faults-live e2e-release-live e2e-live-create e2e-live-run e2e-live-update e2e-live-delete



# 默认目标
help:
	@echo "RustFS Operator Makefile"
	@echo ""
	@echo "Usage:"
	@echo "  make pre-commit        - 执行提交前检查 (Rust + 前端)，与 CI 一致"
	@echo "  make fmt              - 自动格式化 Rust 代码"
	@echo "  make fmt-check        - 检查 Rust 代码格式 (不修改)"
	@echo "  make clippy           - 运行 clippy 检查"
	@echo "  make test             - 运行 Rust 测试"
	@echo "  make build            - 构建项目"
	@echo "  make docker-build-operator  - 构建 operator 镜像 (IMAGE_REPO?=rustfs/operator IMAGE_TAG?=dev)"
	@echo "  make docker-build-console-web - 构建 console-web 前端镜像 (CONSOLE_WEB_IMAGE_REPO?=rustfs/console-web CONSOLE_WEB_IMAGE_TAG?=dev)"
	@echo "  make docker-build-all       - 构建 operator + console-web 两个镜像"
	@echo "  make console-lint     - 前端 ESLint 检查 (console-web)"
	@echo "  make console-fmt     - 前端 Prettier 自动格式化 (console-web)"
	@echo "  make console-fmt-check - 前端 Prettier 格式检查 (console-web)"
	@echo "  make e2e-live-create  - 清理 dedicated storage 后创建 live Kind 环境并加载 e2e 镜像"
	@echo "  make e2e-live-run     - 在已有 live 环境中执行全部 live 用例（smoke/operator/console）"
	@echo "  make e2e-live-update - 根据变更增量重建镜像并更新到 live 环境（load + rollout）"
	@echo "  make e2e-live-delete - 删除 live Kind 环境并清理 dedicated storage"

# 提交前检查：Rust (fmt-check + clippy + test) + 前端 (lint + 格式检查)
pre-commit: fmt-check clippy test console-lint console-fmt-check
	@echo "pre-commit: 所有检查通过"

# 自动格式化
fmt:
	cargo fmt --all

# 检查格式 (CI 使用)
fmt-check:
	cargo fmt --all --check

# Clippy 检查
clippy:
	cargo clippy --all-features -- -D warnings

# 运行测试
test:
	cargo test --all

# 前端 ESLint 检查（需先在 console-web 下执行 npm install）
console-lint:
	cd console-web && npm run lint

# 前端 Prettier 自动格式化（需先在 console-web 下 npm install）
console-fmt:
	cd console-web && npm run format

# 前端 Prettier 格式检查（仅检查不修改）
console-fmt-check:
	cd console-web && npm run format:check

# 构建
build:
	cargo build --release

# Rust-native e2e harness (live-first, dedicated Kind)
E2E_MANIFEST ?= e2e/Cargo.toml
E2E_BIN ?= cargo run --manifest-path $(E2E_MANIFEST) --bin rustfs-e2e --
E2E_TEST_THREADS ?= 1
E2E_LIVE_ENV ?= RUSTFS_E2E_LIVE=1
E2E_BUILD_CACHE_DIR ?= .e2e-cache
E2E_OPERATOR_BUILD_HASH ?= $(E2E_BUILD_CACHE_DIR)/operator-image.sha
E2E_CONSOLE_WEB_BUILD_HASH ?= $(E2E_BUILD_CACHE_DIR)/console-web-image.sha
E2E_FORCE_REBUILD ?= 0

# Core live helpers
# 1) prepare/validate cluster context
# 2) run operator console tests under RUSTFS_E2E_LIVE=1
# All tests are executed as ignored live entrypoints.
e2e-assert-context:
	$(E2E_LIVE_ENV) $(E2E_BIN) assert-context

e2e-kind-create:
	$(E2E_BIN) kind-create

e2e-kind-delete:
	$(E2E_BIN) kind-delete

e2e-kind-load-images:
	$(E2E_BIN) kind-load-images

e2e-storage-prepare:
	$(E2E_LIVE_ENV) $(E2E_BIN) storage-prepare

e2e-deploy-dev:
	$(E2E_LIVE_ENV) $(E2E_BIN) deploy-dev

# Live suites (ignored-by-default in harness)
e2e-smoke-live: e2e-assert-context e2e-deploy-dev
	$(E2E_LIVE_ENV) cargo test --manifest-path $(E2E_MANIFEST) --test smoke -- --ignored --test-threads=$(E2E_TEST_THREADS) --nocapture

e2e-operator-live: e2e-assert-context
	$(E2E_LIVE_ENV) cargo test --manifest-path $(E2E_MANIFEST) --test operator -- --ignored --test-threads=$(E2E_TEST_THREADS) --nocapture

e2e-console-live: e2e-assert-context
	@pf_log=$$(mktemp -t e2e-console-pf-XXXXXXXX.log); \
	$(E2E_LIVE_ENV) kubectl --context kind-rustfs-e2e -n rustfs-system port-forward svc/rustfs-operator-console 19090:9090 >"$$pf_log" 2>&1 & \
	pf_pid=$$!; \
	cleanup() { \
		if [ -n "$$pf_pid" ] && ps -p "$$pf_pid" >/dev/null 2>&1; then \
			kill "$$pf_pid" 2>/dev/null || true; \
			wait "$$pf_pid" 2>/dev/null || true; \
		fi; \
		rm -f "$$pf_log"; \
	}; \
	trap cleanup EXIT INT TERM; \
	retry=20; \
	for i in $$(seq 1 "$$retry"); do \
		if curl -fsS http://127.0.0.1:19090/healthz >/dev/null 2>&1; then \
			break; \
		fi; \
		sleep 1; \
	done; \
	if ! curl -fsS http://127.0.0.1:19090/healthz >/dev/null 2>&1; then \
		echo "console port-forward not ready, see: $$pf_log"; \
		cat "$$pf_log"; \
		exit 1; \
	fi; \
	$(E2E_LIVE_ENV) cargo test --manifest-path $(E2E_MANIFEST) --test console -- --ignored --test-threads=$(E2E_TEST_THREADS) --nocapture

e2e-faults-live: e2e-assert-context
	$(E2E_LIVE_ENV) RUSTFS_E2E_DESTRUCTIVE=1 cargo test --manifest-path $(E2E_MANIFEST) --test faults -- --ignored --test-threads=$(E2E_TEST_THREADS) --nocapture

e2e-release-live: e2e-smoke-live e2e-operator-live e2e-console-live
	@echo "live e2e release subset passed (faults are separate: make e2e-faults-live)."

# 1) 一次创建：构建镜像 -> 清理旧集群和 dedicated storage -> 创建 Kind -> 加载镜像
# 2) 日常：执行用例、更新镜像、删除


e2e-live-create:
	$(MAKE) e2e-build-all
	$(E2E_LIVE_ENV) make e2e-kind-delete || true
	$(E2E_LIVE_ENV) make e2e-kind-create
	$(E2E_LIVE_ENV) make e2e-kind-load-images

# 在已有 live 环境里执行全部 live 用例（smoke/operator/console）
e2e-live-run:
	$(E2E_LIVE_ENV) make e2e-release-live

# 根据最新代码更新镜像：重新 build + load + 重启 deployment（不重建集群）
# 前提：live 集群内已部署 control-plane（常见于 make e2e-live-run 完成后）
e2e-live-update:
	$(MAKE) e2e-build-all
	$(E2E_LIVE_ENV) make e2e-assert-context
	$(E2E_LIVE_ENV) make e2e-kind-load-images
	$(E2E_LIVE_ENV) kubectl --context kind-rustfs-e2e -n rustfs-system rollout restart deployment rustfs-operator rustfs-operator-console rustfs-operator-console-frontend
	$(E2E_LIVE_ENV) kubectl --context kind-rustfs-e2e -n rustfs-system rollout status deployment/rustfs-operator --timeout=180s
	$(E2E_LIVE_ENV) kubectl --context kind-rustfs-e2e -n rustfs-system rollout status deployment/rustfs-operator-console --timeout=180s
	$(E2E_LIVE_ENV) kubectl --context kind-rustfs-e2e -n rustfs-system rollout status deployment/rustfs-operator-console-frontend --timeout=180s

# 删除 dedicated live 集群
# 4) 明确 teardown 入口
e2e-live-delete:
	$(E2E_LIVE_ENV) make e2e-kind-delete

# Build local images with e2e tags, suitable for immediate local smoke/live flows.
e2e-build-all:
	$(MAKE) e2e-build-operator-image
	$(MAKE) e2e-build-console-web-image

# Build operator image only when source has changed (unless forced)
e2e-build-operator-image:
	@mkdir -p $(E2E_BUILD_CACHE_DIR)
	@current_hash=$$(git ls-files -z -- ':(exclude)console-web/*' ':(exclude)e2e/*' | xargs -0 sha256sum 2>/dev/null | sha256sum | cut -d' ' -f1); \
	if [ "$(E2E_FORCE_REBUILD)" = "1" ] || [ ! -f "$(E2E_OPERATOR_BUILD_HASH)" ] || [ "$$current_hash" != "$$(cat "$(E2E_OPERATOR_BUILD_HASH)" 2>/dev/null || true)" ]; then \
		echo "Building operator image (e2e tag): rustfs/operator:e2e"; \
		$(MAKE) docker-build-operator IMAGE_TAG=e2e; \
		printf '%s\n' "$$current_hash" > "$(E2E_OPERATOR_BUILD_HASH)"; \
	else \
		echo "operator image unchanged, skip docker build (use E2E_FORCE_REBUILD=1 to rebuild)"; \
	fi

# Build console-web image only when source has changed (unless forced)
e2e-build-console-web-image:
	@mkdir -p $(E2E_BUILD_CACHE_DIR)
	@current_hash=$$(git ls-files -z -- console-web | xargs -0 sha256sum 2>/dev/null | sha256sum | cut -d' ' -f1); \
	if [ "$(E2E_FORCE_REBUILD)" = "1" ] || [ ! -f "$(E2E_CONSOLE_WEB_BUILD_HASH)" ] || [ "$$current_hash" != "$$(cat "$(E2E_CONSOLE_WEB_BUILD_HASH)" 2>/dev/null || true)" ]; then \
		echo "Building console-web image (e2e tag): rustfs/console-web:e2e"; \
		$(MAKE) docker-build-console-web CONSOLE_WEB_IMAGE_TAG=e2e; \
		printf '%s\n' "$$current_hash" > "$(E2E_CONSOLE_WEB_BUILD_HASH)"; \
	else \
		echo "console-web image unchanged, skip docker build (use E2E_FORCE_REBUILD=1 to rebuild)"; \
	fi


# 构建 Docker 镜像（operator：含 controller + console API；console-web：前端静态资源）
IMAGE_REPO ?= rustfs/operator
IMAGE_TAG  ?= dev
docker-build-operator:
	docker build -t $(IMAGE_REPO):$(IMAGE_TAG) .

CONSOLE_WEB_IMAGE_REPO ?= rustfs/console-web
CONSOLE_WEB_IMAGE_TAG  ?= dev
docker-build-console-web:
	docker build -t $(CONSOLE_WEB_IMAGE_REPO):$(CONSOLE_WEB_IMAGE_TAG) -f console-web/Dockerfile console-web

docker-build-all: docker-build-operator docker-build-console-web
