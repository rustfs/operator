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

# 默认目标
help:
	@echo "RustFS Operator Makefile"
	@echo ""
	@echo "Usage:"
	@echo "  make pre-commit   - 执行提交前检查 (fmt-check + clippy + test)，与 CI 一致"
	@echo "  make fmt         - 自动格式化代码"
	@echo "  make fmt-check   - 检查代码格式 (不修改)"
	@echo "  make clippy      - 运行 clippy 检查"
	@echo "  make test        - 运行测试"
	@echo "  make build       - 构建项目"

# 提交前检查：与 .github/workflows/ci.yml 保持一致
pre-commit: fmt-check clippy test
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

# 构建
build:
	cargo build --release
