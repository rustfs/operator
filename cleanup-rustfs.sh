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

# RustFS Operator cleanup script
# Thorough cleanup: Tenants, Namespace, ClusterRole/ClusterRoleBinding, CRD, local files

set -e

# Color output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Ask for confirmation
confirm_cleanup() {
    if [ "$FORCE" != "true" ]; then
        echo ""
        log_warning "This operation will delete all RustFS resources:"
        echo "  - All Tenants (rustfs-system + cluster-wide)"
        echo "  - Namespace: rustfs-system (Operator, Console, Console Web, Pods, PVCs, Services)"
        echo "  - ClusterRole / ClusterRoleBinding: rustfs-operator, rustfs-operator-console"
        echo "  - CRD: tenants.rustfs.com"
        echo "  - Local generated file: deploy/rustfs-operator/crds/tenant-crd.yaml"
        if [ "$WITH_KIND" = "true" ]; then
            echo "  - Kind cluster: rustfs-dev (Docker container will be removed)"
        fi
        echo ""
        read -p "Confirm deletion? (yes/no): " confirm

        if [ "$confirm" != "yes" ]; then
            log_info "Cleanup cancelled"
            exit 0
        fi
    fi
}

# Delete all Tenants (cluster-wide; CRD must exist)
delete_all_tenants() {
    log_info "Deleting all Tenants (cluster-wide)..."

    if ! kubectl get crd tenants.rustfs.com >/dev/null 2>&1; then
        log_info "CRD tenants.rustfs.com does not exist, no tenants to delete"
        return 0
    fi

    local tenants
    tenants=$(kubectl get tenants --all-namespaces -o name 2>/dev/null) || true
    if [ -z "$tenants" ]; then
        log_info "No tenants found, skipping"
        return 0
    fi

    echo "$tenants" | while read -r line; do
        [ -z "$line" ] && continue
        log_info "Deleting $line..."
        kubectl delete "$line" --timeout=60s 2>/dev/null || kubectl delete "$line" --force --grace-period=0 2>/dev/null || true
    done

    # Wait until no tenants remain
    local timeout=90
    local elapsed=0
    while [ $elapsed -lt $timeout ]; do
        local count
        count=$(kubectl get tenants --all-namespaces -o name 2>/dev/null | wc -l)
        count=$((count + 0))
        if [ "$count" -eq 0 ]; then
            log_success "All tenants deleted"
            return 0
        fi
        sleep 3
        elapsed=$((elapsed + 3))
    done
    log_warning "Some tenants may still be terminating"
}

# Delete Namespace
delete_namespace() {
    log_info "Deleting Namespace: rustfs-system..."

    if kubectl get namespace rustfs-system >/dev/null 2>&1; then
        kubectl delete namespace rustfs-system --timeout=60s

        # Wait for namespace to be deleted
        log_info "Waiting for Namespace to be fully deleted (this may take some time)..."
        local timeout=120
        local elapsed=0
        while kubectl get namespace rustfs-system >/dev/null 2>&1; do
            if [ $elapsed -ge $timeout ]; then
                log_warning "Wait timeout"
                log_info "Namespace may have finalizers preventing deletion, attempting manual cleanup..."

                # Try to remove finalizers
                kubectl get namespace rustfs-system -o json | \
                    jq '.spec.finalizers = []' | \
                    kubectl replace --raw /api/v1/namespaces/rustfs-system/finalize -f - 2>/dev/null || true
                break
            fi
            echo -ne "${BLUE}[INFO]${NC} Waiting for Namespace deletion... ${elapsed}s\r"
            sleep 5
            elapsed=$((elapsed + 5))
        done
        echo "" # New line

        log_success "Namespace deleted"
    else
        log_info "Namespace does not exist, skipping"
    fi
}

# Delete cluster-scoped RBAC (not removed when namespace is deleted)
delete_cluster_rbac() {
    log_info "Deleting ClusterRoleBinding and ClusterRole..."

    for name in rustfs-operator rustfs-operator-console; do
        if kubectl get clusterrolebinding "$name" >/dev/null 2>&1; then
            kubectl delete clusterrolebinding "$name" --timeout=30s 2>/dev/null || true
            log_info "Deleted ClusterRoleBinding: $name"
        fi
        if kubectl get clusterrole "$name" >/dev/null 2>&1; then
            kubectl delete clusterrole "$name" --timeout=30s 2>/dev/null || true
            log_info "Deleted ClusterRole: $name"
        fi
    done

    log_success "Cluster RBAC cleaned"
}

# Delete CRD
delete_crd() {
    log_info "Deleting CRD: tenants.rustfs.com..."

    if kubectl get crd tenants.rustfs.com >/dev/null 2>&1; then
        kubectl delete crd tenants.rustfs.com --timeout=60s

        # Wait for CRD to be deleted
        log_info "Waiting for CRD to be fully deleted..."
        local timeout=60
        local elapsed=0
        while kubectl get crd tenants.rustfs.com >/dev/null 2>&1; do
            if [ $elapsed -ge $timeout ]; then
                log_warning "Wait timeout, forcing deletion..."
                kubectl delete crd tenants.rustfs.com --force --grace-period=0 2>/dev/null || true
                break
            fi
            sleep 2
            elapsed=$((elapsed + 2))
        done

        log_success "CRD deleted"
    else
        log_info "CRD does not exist, skipping"
    fi
}

# Delete Kind cluster (removes the Docker container rustfs-dev-control-plane)
delete_kind_cluster() {
    log_info "Deleting Kind cluster: rustfs-dev..."

    if ! command -v kind >/dev/null 2>&1; then
        log_warning "kind not found in PATH, skipping Kind cluster deletion"
        return 0
    fi

    if kind get clusters 2>/dev/null | grep -q "rustfs-dev"; then
        kind delete cluster --name rustfs-dev
        log_success "Kind cluster rustfs-dev deleted (Docker container removed)"
    else
        log_info "Kind cluster rustfs-dev does not exist, skipping"
    fi
}

# Cleanup local files
cleanup_local_files() {
    log_info "Cleaning up local files..."

    local files_to_clean=(
        "deploy/rustfs-operator/crds/tenant-crd.yaml"
    )

    for file in "${files_to_clean[@]}"; do
        if [ -f "$file" ]; then
            rm -f "$file"
            log_info "Deleted: $file"
        fi
    done

    log_success "Local files cleaned"
}

# Verify cleanup results
verify_cleanup() {
    log_info "Verifying cleanup results..."
    echo ""

    local issues=0

    # Check Tenants (cluster-wide)
    local tenant_count=0
    if kubectl get crd tenants.rustfs.com >/dev/null 2>&1; then
        tenant_count=$(kubectl get tenants --all-namespaces -o name 2>/dev/null | wc -l)
        tenant_count=$((tenant_count + 0))
    fi
    if [ "$tenant_count" -gt 0 ]; then
        log_error "Tenants still exist ($tenant_count)"
        issues=$((issues + 1))
    else
        log_success "✓ Tenants cleaned"
    fi

    # Check Namespace
    if kubectl get namespace rustfs-system >/dev/null 2>&1; then
        log_warning "Namespace still exists (may be terminating)"
        issues=$((issues + 1))
    else
        log_success "✓ Namespace cleaned"
    fi

    # Check CRD
    if kubectl get crd tenants.rustfs.com >/dev/null 2>&1; then
        log_error "CRD still exists"
        issues=$((issues + 1))
    else
        log_success "✓ CRD cleaned"
    fi

    # Check ClusterRole / ClusterRoleBinding
    local rbac_issues=0
    for name in rustfs-operator rustfs-operator-console; do
        kubectl get clusterrolebinding "$name" >/dev/null 2>&1 && rbac_issues=$((rbac_issues + 1))
        kubectl get clusterrole "$name" >/dev/null 2>&1 && rbac_issues=$((rbac_issues + 1))
    done
    if [ $rbac_issues -gt 0 ]; then
        log_error "Cluster RBAC still exists ($rbac_issues resources)"
        issues=$((issues + 1))
    else
        log_success "✓ Cluster RBAC cleaned"
    fi

    echo ""
    if [ $issues -eq 0 ]; then
        log_success "Cleanup verification passed!"
        return 0
    else
        log_warning "Found $issues issue(s), may require manual cleanup"
        return 1
    fi
}

# Show next steps after cleanup
show_next_steps() {
    log_info "=========================================="
    log_info "  Next Steps"
    log_info "=========================================="
    echo ""

    echo "Redeploy:"
    echo "  ./deploy-rustfs.sh"
    echo ""

    echo "Check cluster status:"
    echo "  kubectl get all -n rustfs-system"
    echo "  kubectl get crd tenants.rustfs.com"
    echo ""

    if [ "$WITH_KIND" != "true" ]; then
        echo "Remove Kind cluster and Docker container (optional):"
        echo "  ./cleanup-rustfs.sh -f -k"
        echo "  # or: kind delete cluster --name rustfs-dev"
        echo ""
    fi
}

# Main flow
main() {
    log_info "=========================================="
    log_info "  RustFS Operator Cleanup Script"
    log_info "=========================================="

    confirm_cleanup

    echo ""
    log_info "Starting cleanup..."
    echo ""

    delete_all_tenants
    delete_namespace
    delete_cluster_rbac
    delete_crd
    cleanup_local_files

    if [ "$WITH_KIND" = "true" ]; then
        echo ""
        delete_kind_cluster
    fi

    echo ""
    verify_cleanup

    echo ""
    show_next_steps

    log_success "=========================================="
    log_success "  Cleanup completed!"
    log_success "=========================================="
}

# Parse arguments
FORCE="false"
WITH_KIND="false"
while [[ $# -gt 0 ]]; do
    case $1 in
        -f|--force)
            FORCE="true"
            shift
            ;;
        -k|--with-kind)
            WITH_KIND="true"
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [-f|--force] [-k|--with-kind]"
            echo ""
            echo "Options:"
            echo "  -f, --force      Skip confirmation prompt, force cleanup"
            echo "  -k, --with-kind  Also delete Kind cluster 'rustfs-dev' (removes Docker container)"
            echo "  -h, --help       Show help information"
            echo ""
            echo "Examples:"
            echo "  $0              # Clean K8s resources only, confirm first"
            echo "  $0 -f           # Clean K8s resources, no confirm"
            echo "  $0 -f -k        # Clean K8s resources + delete Kind cluster (no leftover container)"
            exit 0
            ;;
        *)
            log_error "Unknown argument: $1"
            exit 1
            ;;
    esac
done

# Catch Ctrl+C
trap 'log_error "Cleanup interrupted"; exit 1' INT

# 执行主流程
main "$@"
