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
# For complete cleanup of deployed resources for redeployment or testing

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
        echo "  - Tenant: example-tenant"
        echo "  - Namespace: rustfs-system (including all Pods, PVCs, Services)"
        echo "  - CRD: tenants.rustfs.com"
        echo "  - Operator process"
        echo ""
        read -p "Confirm deletion? (yes/no): " confirm

        if [ "$confirm" != "yes" ]; then
            log_info "Cleanup cancelled"
            exit 0
        fi
    fi
}

# Delete Tenant
delete_tenant() {
    log_info "Deleting Tenant..."

    if kubectl get tenant example-tenant -n rustfs-system >/dev/null 2>&1; then
        kubectl delete tenant example-tenant -n rustfs-system --timeout=60s

        # Wait for Tenant to be deleted
        log_info "Waiting for Tenant to be fully deleted..."
        local timeout=60
        local elapsed=0
        while kubectl get tenant example-tenant -n rustfs-system >/dev/null 2>&1; do
            if [ $elapsed -ge $timeout ]; then
                log_warning "Wait timeout, forcing deletion..."
                kubectl delete tenant example-tenant -n rustfs-system --force --grace-period=0 2>/dev/null || true
                break
            fi
            sleep 2
            elapsed=$((elapsed + 2))
        done

        log_success "Tenant deleted"
    else
        log_info "Tenant does not exist, skipping"
    fi
}

# Stop Operator
stop_operator() {
    log_info "Stopping Operator process..."

    # Method 1: Read from PID file
    if [ -f operator.pid ]; then
        local pid=$(cat operator.pid)
        if ps -p $pid > /dev/null 2>&1; then
            log_info "Stopping Operator (PID: $pid)..."
            kill $pid 2>/dev/null || true
            sleep 2

            # If process still exists, force kill
            if ps -p $pid > /dev/null 2>&1; then
                log_warning "Process did not exit normally, forcing termination..."
                kill -9 $pid 2>/dev/null || true
            fi
        fi
        rm -f operator.pid
    fi

    # Method 2: Find all operator processes
    local operator_pids=$(pgrep -f "target/release/operator.*server" 2>/dev/null || true)
    if [ -n "$operator_pids" ]; then
        log_info "Found Operator processes: $operator_pids"
        pkill -f "target/release/operator.*server" || true
        sleep 2

        # Force kill remaining processes
        pkill -9 -f "target/release/operator.*server" 2>/dev/null || true
    fi

    log_success "Operator stopped"
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

# Cleanup local files
cleanup_local_files() {
    log_info "Cleaning up local files..."

    local files_to_clean=(
        "operator.log"
        "operator.pid"
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

    # Check Tenant
    if kubectl get tenant -n rustfs-system 2>/dev/null | grep -q "example-tenant"; then
        log_error "Tenant still exists"
        issues=$((issues + 1))
    else
        log_success "✓ Tenant cleaned"
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

    # Check Operator process
    if pgrep -f "target/release/operator.*server" >/dev/null; then
        log_error "Operator process still running"
        issues=$((issues + 1))
    else
        log_success "✓ Operator stopped"
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

    echo "Completely clean kind cluster (optional):"
    echo "  kind delete cluster --name rustfs-dev"
    echo ""
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

    delete_tenant
    stop_operator
    delete_namespace
    delete_crd
    cleanup_local_files

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
while [[ $# -gt 0 ]]; do
    case $1 in
        -f|--force)
            FORCE="true"
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [-f|--force]"
            echo ""
            echo "Options:"
            echo "  -f, --force    Skip confirmation prompt, force cleanup"
            echo "  -h, --help     Show help information"
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
