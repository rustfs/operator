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

.PHONY: pre-commit fmt fmt-check clippy test build docker-build-operator docker-build-console-web docker-build-all help console-lint console-fmt console-fmt-check

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
