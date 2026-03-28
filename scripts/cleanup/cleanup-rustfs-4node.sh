#!/bin/bash
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

################################################################################
# Cleanup script for the RustFS 4-node demo environment
#
# Removes: Tenants, Namespace, RBAC, CRD, Kind cluster, local storage dirs
# Pair with: deploy-rustfs-4node.sh
#
################################################################################

set -e

# Always run from project root (script cds here; safe to invoke from any cwd)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$PROJECT_ROOT"

CLUSTER_NAME="rustfs-cluster"
OPERATOR_NAMESPACE="rustfs-system"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }
log_warning() { echo -e "${YELLOW}[WARNING]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

confirm_cleanup() {
    if [ "$FORCE" != "true" ]; then
        echo ""
        log_warning "The following will be deleted:"
        echo "  - All Tenants"
        echo "  - Namespace: ${OPERATOR_NAMESPACE}"
        echo "  - ClusterRole / ClusterRoleBinding: rustfs-operator, rustfs-operator-console"
        echo "  - CRD: tenants.rustfs.com"
        echo "  - Kind cluster: ${CLUSTER_NAME}"
        if [ "$CLEAN_STORAGE" = "true" ]; then
            echo "  - Host storage dirs: /tmp/rustfs-storage-{1,2,3}"
        fi
        echo ""
        read -p "Confirm deletion? (yes/no): " confirm
        if [ "$confirm" != "yes" ]; then
            log_info "Cancelled"
            exit 0
        fi
    fi
}

delete_all_tenants() {
    log_info "Deleting all Tenants..."

    if ! kubectl get crd tenants.rustfs.com >/dev/null 2>&1; then
        log_info "CRD not found, skipping"
        return 0
    fi

    local tenants
    tenants=$(kubectl get tenants --all-namespaces -o name 2>/dev/null) || true
    if [ -z "$tenants" ]; then
        log_info "No Tenants, skipping"
        return 0
    fi

    echo "$tenants" | while read -r line; do
        [ -z "$line" ] && continue
        log_info "Deleting $line..."
        kubectl delete "$line" --timeout=60s 2>/dev/null || kubectl delete "$line" --force --grace-period=0 2>/dev/null || true
    done

    local timeout=90
    local elapsed=0
    while [ $elapsed -lt $timeout ]; do
        local count
        count=$(kubectl get tenants --all-namespaces -o name 2>/dev/null | wc -l)
        count=$((count + 0))
        if [ "$count" -eq 0 ]; then
            log_success "Tenants deleted"
            return 0
        fi
        sleep 3
        elapsed=$((elapsed + 3))
    done
    log_warning "Some Tenants may still be terminating"
}

delete_namespace() {
    log_info "Deleting namespace ${OPERATOR_NAMESPACE}..."

    if kubectl get namespace ${OPERATOR_NAMESPACE} >/dev/null 2>&1; then
        kubectl delete namespace ${OPERATOR_NAMESPACE} --timeout=120s

        log_info "Waiting for namespace to be fully removed..."
        local timeout=120
        local elapsed=0
        while kubectl get namespace ${OPERATOR_NAMESPACE} >/dev/null 2>&1; do
            if [ $elapsed -ge $timeout ]; then
                log_warning "Wait timed out"
                kubectl get namespace ${OPERATOR_NAMESPACE} -o json 2>/dev/null | \
                    jq '.spec.finalizers = []' 2>/dev/null | \
                    kubectl replace --raw /api/v1/namespaces/${OPERATOR_NAMESPACE}/finalize -f - 2>/dev/null || true
                break
            fi
            echo -ne "${BLUE}[INFO]${NC} Waiting for namespace deletion... ${elapsed}s\r"
            sleep 5
            elapsed=$((elapsed + 5))
        done
        echo ""
        log_success "Namespace removed"
    else
        log_info "Namespace does not exist, skipping"
    fi
}

delete_cluster_rbac() {
    log_info "Deleting ClusterRoleBinding and ClusterRole..."

    for name in rustfs-operator rustfs-operator-console; do
        kubectl delete clusterrolebinding "$name" --timeout=30s 2>/dev/null || true
        kubectl delete clusterrole "$name" --timeout=30s 2>/dev/null || true
    done

    log_success "RBAC cleaned up"
}

delete_pv_and_storageclass() {
    log_info "Deleting PersistentVolumes and StorageClass..."

    for i in $(seq 1 12); do
        kubectl delete pv rustfs-pv-${i} --timeout=30s 2>/dev/null || true
    done

    kubectl delete storageclass local-storage --timeout=30s 2>/dev/null || true

    log_success "PVs and StorageClass removed"
}

delete_crd() {
    log_info "Deleting CRD tenants.rustfs.com..."

    if kubectl get crd tenants.rustfs.com >/dev/null 2>&1; then
        kubectl delete crd tenants.rustfs.com --timeout=60s

        local timeout=60
        local elapsed=0
        while kubectl get crd tenants.rustfs.com >/dev/null 2>&1; do
            if [ $elapsed -ge $timeout ]; then
                kubectl delete crd tenants.rustfs.com --force --grace-period=0 2>/dev/null || true
                break
            fi
            sleep 2
            elapsed=$((elapsed + 2))
        done
        log_success "CRD deleted"
    else
        log_info "CRD not found, skipping"
    fi
}

delete_kind_cluster() {
    log_info "Deleting Kind cluster ${CLUSTER_NAME}..."

    if ! command -v kind >/dev/null 2>&1; then
        log_warning "kind not found, skipping"
        return 0
    fi

    if kind get clusters 2>/dev/null | grep -q "^${CLUSTER_NAME}$"; then
        kind delete cluster --name ${CLUSTER_NAME}
        log_success "Kind cluster deleted"
    else
        log_info "Kind cluster does not exist, skipping"
    fi
}

cleanup_storage_dirs() {
    log_info "Cleaning host storage directories..."

    for dir in /tmp/rustfs-storage-1 /tmp/rustfs-storage-2 /tmp/rustfs-storage-3; do
        if [ -d "$dir" ]; then
            rm -rf "$dir"
            log_info "Removed $dir"
        fi
    done

    log_success "Storage directories cleaned"
}

cleanup_local_files() {
    log_info "Cleaning generated local files..."

    if [ -f "deploy/rustfs-operator/crds/tenant-crd.yaml" ]; then
        rm -f deploy/rustfs-operator/crds/tenant-crd.yaml
        log_info "Removed tenant-crd.yaml"
    fi

    log_success "Local files cleaned"
}

show_next_steps() {
    echo ""
    log_info "Redeploy with:"
    echo "  ./scripts/deploy/deploy-rustfs-4node.sh"
    echo ""
}

# Parse arguments
FORCE="false"
CLEAN_STORAGE="false"
while [[ $# -gt 0 ]]; do
    case $1 in
        -f|--force)
            FORCE="true"
            shift
            ;;
        -s|--clean-storage)
            CLEAN_STORAGE="true"
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [-f|--force] [-s|--clean-storage]"
            echo ""
            echo "Options:"
            echo "  -f, --force           Skip confirmation"
            echo "  -s, --clean-storage   Also remove host dirs /tmp/rustfs-storage-{1,2,3}"
            echo "  -h, --help            Show this help"
            exit 0
            ;;
        *)
            log_error "Unknown argument: $1"
            exit 1
            ;;
    esac
done

trap 'log_error "Cleanup interrupted"; exit 1' INT

log_info "=========================================="
log_info "  RustFS 4-node environment cleanup"
log_info "=========================================="

confirm_cleanup

echo ""
log_info "Starting cleanup..."
echo ""

# If the cluster exists and is reachable, clean Kubernetes resources first
if kind get clusters 2>/dev/null | grep -q "^${CLUSTER_NAME}$"; then
    kubectl config use-context kind-${CLUSTER_NAME} 2>/dev/null || true
    if kubectl cluster-info >/dev/null 2>&1; then
        delete_all_tenants
        delete_namespace
        delete_cluster_rbac
        delete_pv_and_storageclass
        delete_crd
    fi
else
    log_info "Kind cluster ${CLUSTER_NAME} not found, skipping Kubernetes cleanup"
fi

cleanup_local_files
delete_kind_cluster

if [ "$CLEAN_STORAGE" = "true" ]; then
    cleanup_storage_dirs
fi

echo ""
show_next_steps

log_success "=========================================="
log_success "  Cleanup finished"
log_success "=========================================="
