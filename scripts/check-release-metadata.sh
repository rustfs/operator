#!/usr/bin/env bash
# Copyright 2026 RustFS Team
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

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

cargo_version="$({
  awk '
    /^\[package\]$/ { in_package = 1; next }
    /^\[/ && in_package { exit }
    in_package && $1 == "version" && $2 == "=" {
      gsub(/"/, "", $3)
      print $3
      exit
    }
  ' Cargo.toml
})"
chart_version="$(awk '$1 == "version:" { gsub(/"/, "", $2); print $2; exit }' deploy/rustfs-operator/Chart.yaml)"
app_version="$(awk '$1 == "appVersion:" { gsub(/"/, "", $2); print $2; exit }' deploy/rustfs-operator/Chart.yaml)"
toolchain_version="$({
  awk -F= '
    $1 ~ /^[[:space:]]*channel[[:space:]]*$/ {
      gsub(/[[:space:]\"]/, "", $2)
      print $2
      exit
    }
  ' rust-toolchain.toml
})"
setup_version="$({
  awk '
    /^  rust-version:$/ { in_input = 1; next }
    in_input && $1 == "default:" {
      gsub(/"/, "", $2)
      print $2
      exit
    }
  ' .github/actions/setup/action.yml
})"
ci_version="$({
  awk '
    /name: Setup Rust environment/ { in_setup = 1; next }
    in_setup && $1 == "rust-version:" { print $2; exit }
  ' .github/workflows/ci.yml
})"
docker_rust_version="$({
  awk -F= '
    /^ARG RUST_BUILD_IMAGE=rust:/ {
      if ($2 == "rust:bookworm") {
        print "stable"
      } else {
        sub(/^rust:/, "", $2)
        sub(/-bookworm$/, "", $2)
        print $2
      }
      exit
    }
  ' Dockerfile
})"

lock_package_version() {
  local lockfile="$1"
  awk '
    $0 == "name = \"operator\"" { in_operator = 1; next }
    in_operator && $1 == "version" && $2 == "=" {
      gsub(/"/, "", $3)
      print $3
      exit
    }
    in_operator && /^\[\[package\]\]$/ { in_operator = 0 }
  ' "$lockfile"
}

root_lock_version="$(lock_package_version Cargo.lock)"
e2e_lock_version="$(lock_package_version e2e/Cargo.lock)"

require_value() {
  local name="$1"
  local value="$2"
  if [[ -z "$value" ]]; then
    echo "release metadata check failed: could not read $name" >&2
    exit 1
  fi
}

require_equal() {
  local name="$1"
  local actual="$2"
  local expected="$3"
  if [[ "$actual" != "$expected" ]]; then
    echo "release metadata check failed: $name is '$actual', expected '$expected'" >&2
    exit 1
  fi
}

require_value "Cargo package version" "$cargo_version"
require_value "Helm chart version" "$chart_version"
require_value "Helm appVersion" "$app_version"
require_value "Rust toolchain version" "$toolchain_version"
require_value "setup action Rust version" "$setup_version"
require_value "CI Rust version" "$ci_version"
require_value "Docker Rust version" "$docker_rust_version"
require_value "Cargo.lock Operator version" "$root_lock_version"
require_value "e2e/Cargo.lock Operator version" "$e2e_lock_version"

if [[ ! "$cargo_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]]; then
  echo "release metadata check failed: '$cargo_version' is not a supported release version" >&2
  exit 1
fi

tenant_chart_version="$(awk '$1 == "version:" { gsub(/"/, "", $2); print $2; exit }' deploy/rustfs-tenant/Chart.yaml)"
require_value "Tenant chart version" "$tenant_chart_version"
require_equal "Tenant chart version" "$tenant_chart_version" "$cargo_version"

require_equal "Helm chart version" "$chart_version" "$cargo_version"
require_equal "Helm appVersion" "$app_version" "$cargo_version"
require_equal "setup action Rust version" "$setup_version" "$toolchain_version"
require_equal "CI Rust version" "$ci_version" "$toolchain_version"
require_equal "Docker Rust version" "$docker_rust_version" "$toolchain_version"
require_equal "Cargo.lock Operator version" "$root_lock_version" "$cargo_version"
require_equal "e2e/Cargo.lock Operator version" "$e2e_lock_version" "$cargo_version"

grep -Fq 'ENV RUSTFS_OPERATOR_VERSION=${VERSION}' Dockerfile || {
  echo "release metadata check failed: Docker does not pass VERSION to the Rust build" >&2
  exit 1
}
grep -Fq '.Values.operator.image.tag | default .Chart.AppVersion' deploy/rustfs-operator/templates/deployment.yaml || {
  echo "release metadata check failed: Operator image does not default to Chart.appVersion" >&2
  exit 1
}
grep -Fq '.Values.console.image.tag | default .Values.operator.image.tag | default .Chart.AppVersion' \
  deploy/rustfs-operator/templates/console-deployment.yaml || {
  echo "release metadata check failed: Console image does not default to Chart.appVersion" >&2
  exit 1
}
grep -Fq -- '--app-version "${{ steps.version.outputs.chart_version }}"' .github/workflows/helm-package.yml || {
  echo "release metadata check failed: Helm packaging does not set appVersion from the release tag" >&2
  exit 1
}

echo "release metadata is consistent: operator=$cargo_version rust=$toolchain_version"
