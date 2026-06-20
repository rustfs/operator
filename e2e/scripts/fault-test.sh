#!/usr/bin/env bash
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

set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PACKAGE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
MANIFEST="$PACKAGE_DIR/Cargo.toml"
MANAGER="rustfs-operator-fault-test"
MANAGER_SELECTOR="app.kubernetes.io/managed-by=$MANAGER"
DEFAULT_SCENARIOS="io-eio pod-kill-one network-partition-one io-read-mistake disk-full warp-under-chaos"
EXPECTED_OBJECTS=40000
EXPECTED_CONCURRENCY=80
EXPECTED_PAYLOAD_BYTES=20337459200
BUILD_JOBS="${RUSTFS_FAULT_TEST_BUILD_JOBS:-1}"

FAULT_NAMESPACE="${RUSTFS_FAULT_TEST_NAMESPACE:-rustfs-fault-test}"
FAULT_TENANT="${RUSTFS_FAULT_TEST_TENANT:-fault-test-tenant}"
CHAOS_NAMESPACE="${RUSTFS_FAULT_TEST_CHAOS_NAMESPACE:-chaos-mesh}"
ACTIVE_PID=""
ACTIVE_ARTIFACTS=""
FAULT_TEST_BINARY=""

usage() {
  cat <<'EOF'
Usage: fault-test.sh <command> [scenario]

Commands:
  preflight [scenario]  Validate the current real-cluster environment.
  run <scenario>        Run one destructive scenario with health guards.
  run-regular           Run the six regular scenarios serially.
  cleanup               Remove managed Chaos and the owned fault namespace.

Run through the package Make targets documented in e2e/FAULT_TESTING.md.
EOF
}

die() {
  echo "fault-test: $*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

kubectl_context() {
  kubectl config current-context
}

kubectl_ns() {
  kubectl --context "$RUSTFS_FAULT_TEST_EXPECTED_CONTEXT" -n "$1" "${@:2}"
}

kubectl_cluster() {
  kubectl --context "$RUSTFS_FAULT_TEST_EXPECTED_CONTEXT" "$@"
}

is_supported_scenario() {
  case "$1" in
    io-eio|pod-kill-one|network-partition-one|io-read-mistake|disk-full|warp-under-chaos|dm-flakey)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

scenario_crd() {
  case "$1" in
    pod-kill-one) echo "podchaos.chaos-mesh.org" ;;
    network-partition-one) echo "networkchaos.chaos-mesh.org" ;;
    dm-flakey) echo "" ;;
    *) echo "iochaos.chaos-mesh.org" ;;
  esac
}

require_namespace_ownership() {
  if ! kubectl_cluster get namespace "$FAULT_NAMESPACE" >/dev/null 2>&1; then
    return 0
  fi

  local manager tenant
  manager="$(kubectl_cluster get namespace "$FAULT_NAMESPACE" -o jsonpath='{.metadata.labels.app\.kubernetes\.io/managed-by}')"
  tenant="$(kubectl_cluster get namespace "$FAULT_NAMESPACE" -o jsonpath='{.metadata.annotations.rustfs\.com/fault-test-tenant}')"
  [[ "$manager" == "$MANAGER" ]] || die "namespace $FAULT_NAMESPACE is not managed by $MANAGER"
  [[ "$tenant" == "$FAULT_TENANT" ]] || die "namespace $FAULT_NAMESPACE is not owned by tenant $FAULT_TENANT"
}

prepare_fault_binary() {
  local scenario="$1" run_root="$2"
  local build_messages="$run_root/fault-build.jsonl"
  local -a build_command=(
    cargo test --manifest-path "$MANIFEST" --test faults --no-run
    --message-format=json-render-diagnostics
  )

  [[ "$BUILD_JOBS" =~ ^[1-9][0-9]*$ ]] || die "RUSTFS_FAULT_TEST_BUILD_JOBS must be a positive integer"
  preflight "$scenario"
  echo "preparing fault-test binary with jobs=$BUILD_JOBS and lowest host priority"
  if command -v ionice >/dev/null 2>&1; then
    CARGO_BUILD_JOBS="$BUILD_JOBS" nice -n 19 ionice -c3 "${build_command[@]}" \
      >"$build_messages" 2>"$run_root/fault-build.log"
  else
    CARGO_BUILD_JOBS="$BUILD_JOBS" nice -n 19 "${build_command[@]}" \
      >"$build_messages" 2>"$run_root/fault-build.log"
  fi
  FAULT_TEST_BINARY="$(jq -r '
    select(
      .reason == "compiler-artifact"
      and .target.name == "faults"
      and (.target.kind | index("test"))
    )
    | .executable // empty
  ' "$build_messages" | tail -n 1)"
  [[ -x "$FAULT_TEST_BINARY" ]] || die "faults test binary was not produced; see $run_root/fault-build.log"
  printf '%s\n' "$FAULT_TEST_BINARY" >"$run_root/fault-test-binary.path"

  preflight "$scenario"
  echo "fault-test binary ready"
}

chaos_deployment_ready() {
  kubectl_ns "$CHAOS_NAMESPACE" get deployment chaos-controller-manager -o json | jq -r '
    (.status.readyReplicas // 0) == (.spec.replicas // 0) and (.spec.replicas // 0) > 0
  '
}

chaos_daemon_ready() {
  kubectl_ns "$CHAOS_NAMESPACE" get daemonset chaos-daemon -o json | jq -r '
    (.status.numberReady // 0) == (.status.desiredNumberScheduled // 0) and (.status.desiredNumberScheduled // 0) > 0
  '
}

chaos_is_ready() {
  local deployment_ready daemon_ready
  deployment_ready="$(chaos_deployment_ready 2>/dev/null)" || return 1
  daemon_ready="$(chaos_daemon_ready 2>/dev/null)" || return 1
  [[ "$deployment_ready" == "true" && "$daemon_ready" == "true" ]]
}

require_chaos_ready() {
  local deployment_ready daemon_ready
  deployment_ready="$(chaos_deployment_ready)"
  daemon_ready="$(chaos_daemon_ready)"
  [[ "$deployment_ready" == "true" ]] || die "Chaos Mesh controller-manager is not fully Ready"
  [[ "$daemon_ready" == "true" ]] || die "Chaos Mesh chaos-daemon is not fully Ready"
}

require_storage_class() {
  local scenario="$1"
  local storage_class provisioner pv_count
  storage_class="${RUSTFS_FAULT_TEST_STORAGE_CLASS:-}"
  [[ -n "$storage_class" ]] || die "RUSTFS_FAULT_TEST_STORAGE_CLASS is required"
  provisioner="$(kubectl_cluster get storageclass "$storage_class" -o json | jq -r '.provisioner // ""')"
  [[ -n "$provisioner" ]] || die "StorageClass $storage_class has no provisioner"

  if [[ "$scenario" == "dm-flakey" ]]; then
    [[ "$provisioner" == "kubernetes.io/no-provisioner" ]] || die "dm-flakey requires a no-provisioner StorageClass"
    pv_count="$(kubectl_cluster get pv -o json | jq -r --arg storage_class "$storage_class" '
      [.items[]
        | select(.spec.storageClassName == $storage_class)
        | select(.status.phase == "Available" or .status.phase == "Bound")
        | select(.spec.capacity.storage == "100Gi")]
      | length
    ')"
    [[ "$pv_count" -eq 4 ]] || die "dm-flakey requires exactly four Available/Bound 100Gi PVs, found $pv_count"
  else
    [[ "$provisioner" != "kubernetes.io/no-provisioner" ]] || die "regular scenarios require dynamic provisioning"
  fi
}

preflight() {
  local scenario="${1:-io-eio}"
  local current_context ready_nodes crd
  is_supported_scenario "$scenario" || die "unsupported scenario: $scenario"

  require_command cargo
  require_command jq
  require_command kubectl
  require_command nice
  require_command pgrep
  [[ -n "${RUSTFS_FAULT_TEST_EXPECTED_CONTEXT:-}" ]] || die "RUSTFS_FAULT_TEST_EXPECTED_CONTEXT is required"
  [[ -n "${RUSTFS_FAULT_TEST_SERVER_IMAGE:-}" ]] || die "RUSTFS_FAULT_TEST_SERVER_IMAGE is required"

  current_context="$(kubectl_context)"
  [[ "$current_context" == "$RUSTFS_FAULT_TEST_EXPECTED_CONTEXT" ]] || die "current context $current_context does not match expected context $RUSTFS_FAULT_TEST_EXPECTED_CONTEXT"
  [[ "$current_context" != kind-* ]] || die "fault tests require a real Kubernetes cluster, got $current_context"

  kubectl_cluster get crd tenants.rustfs.com >/dev/null
  ready_nodes="$(kubectl_cluster get nodes -o json | jq -r '[.items[]
    | select(.spec.unschedulable != true)
    | select(any(.status.conditions[]; .type == "Ready" and .status == "True"))] | length')"
  [[ "$ready_nodes" -ge 4 ]] || die "at least four schedulable Ready nodes are required, found $ready_nodes"

  require_storage_class "$scenario"
  require_namespace_ownership

  if [[ "$scenario" != "dm-flakey" ]]; then
    crd="$(scenario_crd "$scenario")"
    kubectl_cluster get crd "$crd" >/dev/null
    require_chaos_ready
  fi
  if [[ "$scenario" == "warp-under-chaos" ]]; then
    require_command warp
  fi
  if [[ "$scenario" == "dm-flakey" ]]; then
    [[ -n "${RUSTFS_FAULT_TEST_DM_NAME:-}" ]] || die "RUSTFS_FAULT_TEST_DM_NAME is required"
    [[ -n "${RUSTFS_FAULT_TEST_DM_NODE:-}" ]] || die "RUSTFS_FAULT_TEST_DM_NODE is required"
    [[ -n "${RUSTFS_FAULT_TEST_DM_MOUNT_PATH:-}" ]] || die "RUSTFS_FAULT_TEST_DM_MOUNT_PATH is required"
    [[ -n "${RUSTFS_FAULT_TEST_DM_FAULT_TABLE:-}" ]] || die "RUSTFS_FAULT_TEST_DM_FAULT_TABLE is required"
    kubectl_cluster get namespace "$FAULT_NAMESPACE" >/dev/null 2>&1 || die "dm-flakey requires a pre-created owned fault namespace with privileged Pod Security"
    [[ "$(kubectl_cluster get namespace "$FAULT_NAMESPACE" -o jsonpath='{.metadata.labels.pod-security\.kubernetes\.io/enforce}')" == "privileged" ]] || die "dm-flakey requires pod-security.kubernetes.io/enforce=privileged on $FAULT_NAMESPACE"
  fi

  echo "preflight passed: context=$current_context scenario=$scenario nodes=$ready_nodes storageClass=${RUSTFS_FAULT_TEST_STORAGE_CLASS}"
}

preflight_cleanup() {
  local current_context
  require_command jq
  require_command kubectl
  [[ -n "${RUSTFS_FAULT_TEST_EXPECTED_CONTEXT:-}" ]] || die "RUSTFS_FAULT_TEST_EXPECTED_CONTEXT is required"
  current_context="$(kubectl_context)"
  [[ "$current_context" == "$RUSTFS_FAULT_TEST_EXPECTED_CONTEXT" ]] || die "current context $current_context does not match expected context $RUSTFS_FAULT_TEST_EXPECTED_CONTEXT"
  [[ "$current_context" != kind-* ]] || die "fault cleanup requires a real Kubernetes cluster, got $current_context"
  require_namespace_ownership
}

cleanup_managed_chaos() {
  kubectl_ns "$CHAOS_NAMESPACE" delete iochaos,podchaos,networkchaos \
    -l "$MANAGER_SELECTOR" --ignore-not-found=true --wait=false >/dev/null 2>&1 || true
}

terminate_process_tree() {
  local parent="$1"
  local child
  for child in $(pgrep -P "$parent" 2>/dev/null || true); do
    terminate_process_tree "$child"
  done
  kill -TERM "$parent" 2>/dev/null || true
}

handle_signal() {
  cleanup_managed_chaos
  if [[ -n "$ACTIVE_PID" ]]; then
    terminate_process_tree "$ACTIVE_PID"
  fi
  if [[ -n "$ACTIVE_ARTIFACTS" ]]; then
    touch "$ACTIVE_ARTIFACTS/interrupted"
    echo 130 >"$ACTIVE_ARTIFACTS/exit-code"
    capture_cluster_snapshot "$ACTIVE_ARTIFACTS" interrupted
    capture_fault_logs "$ACTIVE_ARTIFACTS"
  fi
  exit 130
}

capture_cluster_snapshot() {
  local artifacts="$1" stage="$2"
  kubectl_cluster get nodes -o wide >"$artifacts/nodes-$stage.txt" 2>&1 || true
  kubectl_cluster get tenants -A -o wide >"$artifacts/tenants-$stage.txt" 2>&1 || true
  kubectl_cluster get pods -A -o wide >"$artifacts/pods-$stage.txt" 2>&1 || true
  kubectl_cluster get pv,pvc -A -o wide >"$artifacts/volumes-$stage.txt" 2>&1 || true
  kubectl_ns "$CHAOS_NAMESPACE" get iochaos,podchaos,networkchaos -o yaml >"$artifacts/chaos-$stage.yaml" 2>&1 || true
  kubectl_ns "$FAULT_NAMESPACE" get events --sort-by=.lastTimestamp >"$artifacts/events-$stage.txt" 2>&1 || true
}

capture_fault_logs() {
  local artifacts="$1" pod name
  for pod in $(kubectl_ns "$FAULT_NAMESPACE" get pods -l "rustfs.tenant=$FAULT_TENANT" -o name 2>/dev/null || true); do
    name="${pod#pod/}"
    kubectl_ns "$FAULT_NAMESPACE" logs "$pod" >"$artifacts/$name.log" 2>&1 || true
    kubectl_ns "$FAULT_NAMESPACE" logs "$pod" --previous >"$artifacts/$name-previous.log" 2>&1 || true
  done
}

health_is_safe() {
  local baseline_ready_nodes="$1" require_chaos="$2"
  local current_ready_nodes
  current_ready_nodes="$(kubectl_cluster get nodes -o json 2>/dev/null | jq -r '[.items[] | select(any(.status.conditions[]; .type == "Ready" and .status == "True"))] | length' 2>/dev/null || echo 0)"
  [[ "$current_ready_nodes" -ge "$baseline_ready_nodes" ]] || return 1
  [[ "$require_chaos" != "true" ]] || chaos_is_ready || return 1
  return 0
}

find_artifact() {
  find "$1" -name "$2" -type f -print -quit
}

validate_scenario_artifacts() {
  local scenario="$1" artifacts="$2" run_root="$3"
  local metadata plan evidence checker summary recommit seed disruptions recommitted committed
  metadata="$(find_artifact "$artifacts" run-metadata.json)"
  plan="$(find_artifact "$artifacts" workload-plan.json)"
  evidence="$(find_artifact "$artifacts" fault-evidence.json)"
  checker="$(find_artifact "$artifacts" checker-report.json)"
  summary="$(find_artifact "$artifacts" workload-summary.json)"
  recommit="$(find_artifact "$artifacts" recommit-report.json)"
  [[ -f "$metadata" ]] || die "$scenario did not produce run-metadata.json"
  [[ -f "$plan" ]] || die "$scenario did not produce workload-plan.json"
  [[ -f "$evidence" ]] || die "$scenario did not produce fault-evidence.json"
  [[ -f "$checker" ]] || die "$scenario did not produce checker-report.json"
  [[ -f "$summary" ]] || die "$scenario did not produce workload-summary.json"
  [[ -f "$recommit" ]] || die "$scenario did not produce recommit-report.json"

  jq -e --arg scenario "$scenario" '
    .scenario == $scenario
    and (.run_id | length) > 0
    and (.rustfs_image | length) > 0
    and (.storage_class | length) > 0
    and (.context | length) > 0
  ' "$metadata" >/dev/null || die "$scenario run metadata is incomplete"
  jq -e --argjson objects "$EXPECTED_OBJECTS" --argjson concurrency "$EXPECTED_CONCURRENCY" --argjson payload "$EXPECTED_PAYLOAD_BYTES" '
    .object_count == $objects
    and .concurrency == $concurrency
    and .total_payload_bytes == $payload
    and .size_distribution == [
      {"size_bytes":4096,"object_count":34000},
      {"size_bytes":16384,"object_count":4000},
      {"size_bytes":8388608,"object_count":1600},
      {"size_bytes":16777216,"object_count":400}
    ]
  ' "$plan" >/dev/null || die "$scenario workload plan does not match the required profile"
  jq -e '.injected == true and .active_during_workload == true and .recovered == true' "$evidence" >/dev/null || die "$scenario fault evidence is incomplete"
  jq -e '(.active_snapshots | length) > 0 and (.workload_snapshots | length) > 0' "$evidence" >/dev/null || die "$scenario fault evidence snapshots are missing"
  jq -e --argjson objects "$EXPECTED_OBJECTS" '
    .committed_puts == $objects
    and (.missing_committed_objects | length) == 0
    and (.hash_mismatches | length) == 0
    and (.successful_corrupted_reads | length) == 0
    and (.list_warnings | length) == 0
    and .tenant_recovered == true
    and .passed == true
  ' "$checker" >/dev/null || die "$scenario checker verdict failed"
  jq -e '
    .attempted == .committed
    and .failed == 0
    and (.attempts | length) == .attempted
  ' "$recommit" >/dev/null || die "$scenario recovery recommit report contains failed attempts"

  seed="$(jq -r '.seed' "$plan")"
  disruptions="$(jq -r '.client_disruptions' "$evidence")"
  recommitted="$(jq -r '.committed' "$recommit")"
  committed="$(jq -r '.committed_puts' "$checker")"
  printf '%s\t%s\t0\t%s\t%s\t%s\t0\t0\t0\t0\ttrue\n' \
    "$scenario" "$seed" "$disruptions" "$recommitted" "$committed" >>"$run_root/validation-summary.tsv"
}

write_runner_failure_summary() {
  local scenario="$1" artifacts="$2" rc="$3"
  local health_guard_failed=false rust_failure_summary=false
  [[ ! -f "$artifacts/health-guard-failed" ]] || health_guard_failed=true
  [[ ! -f "$artifacts/failure-summary.json" ]] || rust_failure_summary=true
  jq -n \
    --arg scenario "$scenario" \
    --argjson exit_code "$rc" \
    --argjson health_guard_failed "$health_guard_failed" \
    --argjson rust_failure_summary "$rust_failure_summary" \
    --arg test_log "$artifacts/test.log" \
    '{
      scenario: $scenario,
      stage: "runner",
      exit_code: $exit_code,
      health_guard_failed: $health_guard_failed,
      rust_failure_summary_present: $rust_failure_summary,
      test_log: $test_log
    }' >"$artifacts/runner-failure-summary.json"
}

run_scenario() {
  local scenario="$1" run_root="$2"
  local artifacts="$run_root/$scenario"
  local baseline_ready_nodes test_pid rc current_time health_checks require_chaos
  preflight "$scenario"
  mkdir -p "$artifacts"
  baseline_ready_nodes="$(kubectl_cluster get nodes -o json | jq -r '[.items[] | select(any(.status.conditions[]; .type == "Ready" and .status == "True"))] | length')"
  if [[ "$scenario" == "dm-flakey" ]]; then
    require_chaos=false
  else
    require_chaos=true
  fi
  capture_cluster_snapshot "$artifacts" before

  echo "starting scenario=$scenario artifacts=$artifacts"
  (
    set +e
    RUSTFS_FAULT_TEST_DESTRUCTIVE=1 \
    RUSTFS_FAULT_TEST_SCENARIO="$scenario" \
    RUSTFS_FAULT_TEST_WORKLOAD_OBJECTS="$EXPECTED_OBJECTS" \
    RUSTFS_FAULT_TEST_WORKLOAD_CONCURRENCY="$EXPECTED_CONCURRENCY" \
    RUSTFS_FAULT_TEST_DURATION_SECONDS="${RUSTFS_FAULT_TEST_DURATION_SECONDS:-7200}" \
    RUSTFS_FAULT_TEST_ARTIFACTS="$artifacts" \
    "$FAULT_TEST_BINARY" --ignored --test-threads=1 --nocapture \
      >"$artifacts/test.log" 2>&1
    echo "$?" >"$artifacts/test-exit-code.tmp"
  ) &
  test_pid=$!
  ACTIVE_PID="$test_pid"
  ACTIVE_ARTIFACTS="$artifacts"
  health_checks=0

  while kill -0 "$test_pid" 2>/dev/null; do
    current_time="$(date -u +%FT%TZ)"
    health_checks=$((health_checks + 1))
    if health_is_safe "$baseline_ready_nodes" "$require_chaos"; then
      echo "$current_time safe=true" >>"$artifacts/health-watch.log"
      if (( health_checks % 6 == 0 )); then
        echo "scenario=$scenario running safe=true time=$current_time"
      fi
    else
      echo "$current_time safe=false" >>"$artifacts/health-watch.log"
      touch "$artifacts/health-guard-failed"
      cleanup_managed_chaos
      terminate_process_tree "$test_pid"
      break
    fi
    sleep 10
  done

  wait "$test_pid" 2>/dev/null || true
  ACTIVE_PID=""
  ACTIVE_ARTIFACTS=""
  rc=125
  [[ -f "$artifacts/test-exit-code.tmp" ]] && rc="$(cat "$artifacts/test-exit-code.tmp")"
  [[ ! -f "$artifacts/health-guard-failed" ]] || rc=90
  echo "$rc" >"$artifacts/exit-code"
  capture_cluster_snapshot "$artifacts" after
  capture_fault_logs "$artifacts"

  if [[ "$rc" -ne 0 ]]; then
    write_runner_failure_summary "$scenario" "$artifacts" "$rc"
    cleanup_managed_chaos
    echo "scenario failed: $scenario rc=$rc log=$artifacts/test.log" >&2
    return "$rc"
  fi
  validate_scenario_artifacts "$scenario" "$artifacts" "$run_root"
  echo "scenario passed: $scenario"
}

new_run_root() {
  if [[ -n "${RUSTFS_FAULT_TEST_RUN_ROOT:-}" ]]; then
    echo "$RUSTFS_FAULT_TEST_RUN_ROOT"
  else
    echo "$PACKAGE_DIR/target/fault-tests/$(date -u +%Y%m%dT%H%M%SZ)"
  fi
}

initialize_summary() {
  local run_root="$1"
  mkdir -p "$run_root"
  if [[ ! -f "$run_root/validation-summary.tsv" ]]; then
    printf 'scenario\tseed\texit\tdisruptions\trecommitted\tcommitted\tmissing\thash_mismatch\tcorrupt_read\tlist_warning\trecovered\n' \
      >"$run_root/validation-summary.tsv"
  fi
}

run_one() {
  local scenario="$1" run_root
  is_supported_scenario "$scenario" || die "unsupported scenario: $scenario"
  run_root="$(new_run_root)"
  initialize_summary "$run_root"
  prepare_fault_binary "$scenario" "$run_root"
  run_scenario "$scenario" "$run_root"
  echo "run artifacts: $run_root"
}

run_regular() {
  local run_root scenario prepared=false
  local scenarios="${RUSTFS_FAULT_TEST_SCENARIOS:-$DEFAULT_SCENARIOS}"
  run_root="$(new_run_root)"
  initialize_summary "$run_root"
  for scenario in $scenarios; do
    [[ "$scenario" != "dm-flakey" ]] || die "run-regular cannot include dm-flakey"
    if [[ "$prepared" == "false" ]]; then
      prepare_fault_binary "$scenario" "$run_root"
      prepared=true
    fi
    run_scenario "$scenario" "$run_root" || return $?
  done
  echo "regular scenario artifacts: $run_root"
}

cleanup() {
  cleanup_managed_chaos
  if kubectl_cluster get namespace "$FAULT_NAMESPACE" >/dev/null 2>&1; then
    require_namespace_ownership
    kubectl_cluster delete namespace "$FAULT_NAMESPACE" --wait=true
  fi
  if kubectl_ns "$CHAOS_NAMESPACE" get iochaos,podchaos,networkchaos -l "$MANAGER_SELECTOR" -o name 2>/dev/null | grep -q .; then
    die "managed Chaos resources remain after cleanup"
  fi
  echo "managed fault-test resources cleaned; external StorageClasses, PVs, and host devices were not changed"
}

trap handle_signal INT TERM HUP

case "${1:-help}" in
  help|-h|--help)
    usage
    ;;
  preflight)
    preflight "${2:-io-eio}"
    ;;
  run)
    [[ -n "${2:-}" ]] || die "scenario is required"
    run_one "$2"
    ;;
  run-regular)
    run_regular
    ;;
  cleanup)
    preflight_cleanup
    cleanup
    ;;
  *)
    usage >&2
    die "unknown command: $1"
    ;;
esac
