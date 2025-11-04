# Copyright 2025 RustFS Team
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http:#www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# Format all code
fmt:
    cargo fmt --all

# Check if code is properly formatted
fmt-check:
    cargo fmt --all --check

# Run clippy checks
clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# Run compilation check
check:
    cargo check --all-targets

# Run tests
test:
    cargo nextest run

# Run all pre-commit checks (format + clippy + check + test)
pre-commit: fmt clippy check test

# Build the operator binary
build:
    cargo build --release

# Build Docker image
build-image tag:
    docker buildx build --platform linux/amd64,linux/arm64 -t rustfs/operator:{{ tag }} .
