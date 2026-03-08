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
# RustFS 4-node 部署环境清理脚本
#
# 清理: Tenants, Namespace, RBAC, CRD, Kind 集群, 本地存储目录
# 与 deploy-rustfs-4node.sh 配套使用
#
################################################################################

set -e

# 保证从项目根目录执行（可从任意位置调用本脚本）
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$PROJECT_ROOT"

CLUSTER_NAME="rustfs-cluster"
OPERATOR_NAMESPACE="rustfs-system"

# 颜色
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
        log_warning "将删除以下资源:"
        echo "  - 所有 Tenants"
        echo "  - 命名空间: ${OPERATOR_NAMESPACE}"
        echo "  - ClusterRole / ClusterRoleBinding: rustfs-operator, rustfs-operator-console"
        echo "  - CRD: tenants.rustfs.com"
        echo "  - Kind 集群: ${CLUSTER_NAME}"
        if [ "$CLEAN_STORAGE" = "true" ]; then
            echo "  - 主机存储目录: /tmp/rustfs-storage-{1,2,3}"
        fi
        echo ""
        read -p "确认删除? (yes/no): " confirm
        if [ "$confirm" != "yes" ]; then
            log_info "已取消"
            exit 0
        fi
    fi
}

delete_all_tenants() {
    log_info "删除所有 Tenants..."

    if ! kubectl get crd tenants.rustfs.com >/dev/null 2>&1; then
        log_info "CRD 不存在，跳过"
        return 0
    fi

    local tenants
    tenants=$(kubectl get tenants --all-namespaces -o name 2>/dev/null) || true
    if [ -z "$tenants" ]; then
        log_info "无 Tenant，跳过"
        return 0
    fi

    echo "$tenants" | while read -r line; do
        [ -z "$line" ] && continue
        log_info "删除 $line..."
        kubectl delete "$line" --timeout=60s 2>/dev/null || kubectl delete "$line" --force --grace-period=0 2>/dev/null || true
    done

    local timeout=90
    local elapsed=0
    while [ $elapsed -lt $timeout ]; do
        local count
        count=$(kubectl get tenants --all-namespaces -o name 2>/dev/null | wc -l)
        count=$((count + 0))
        if [ "$count" -eq 0 ]; then
            log_success "Tenants 已删除"
            return 0
        fi
        sleep 3
        elapsed=$((elapsed + 3))
    done
    log_warning "部分 Tenant 可能仍在终止中"
}

delete_namespace() {
    log_info "删除命名空间 ${OPERATOR_NAMESPACE}..."

    if kubectl get namespace ${OPERATOR_NAMESPACE} >/dev/null 2>&1; then
        kubectl delete namespace ${OPERATOR_NAMESPACE} --timeout=120s

        log_info "等待命名空间完全删除..."
        local timeout=120
        local elapsed=0
        while kubectl get namespace ${OPERATOR_NAMESPACE} >/dev/null 2>&1; do
            if [ $elapsed -ge $timeout ]; then
                log_warning "等待超时"
                kubectl get namespace ${OPERATOR_NAMESPACE} -o json 2>/dev/null | \
                    jq '.spec.finalizers = []' 2>/dev/null | \
                    kubectl replace --raw /api/v1/namespaces/${OPERATOR_NAMESPACE}/finalize -f - 2>/dev/null || true
                break
            fi
            echo -ne "${BLUE}[INFO]${NC} 等待命名空间删除... ${elapsed}s\r"
            sleep 5
            elapsed=$((elapsed + 5))
        done
        echo ""
        log_success "命名空间已删除"
    else
        log_info "命名空间不存在，跳过"
    fi
}

delete_cluster_rbac() {
    log_info "删除 ClusterRoleBinding 和 ClusterRole..."

    for name in rustfs-operator rustfs-operator-console; do
        kubectl delete clusterrolebinding "$name" --timeout=30s 2>/dev/null || true
        kubectl delete clusterrole "$name" --timeout=30s 2>/dev/null || true
    done

    log_success "RBAC 已清理"
}

delete_pv_and_storageclass() {
    log_info "删除 PersistentVolumes 和 StorageClass..."

    for i in $(seq 1 12); do
        kubectl delete pv rustfs-pv-${i} --timeout=30s 2>/dev/null || true
    done

    kubectl delete storageclass local-storage --timeout=30s 2>/dev/null || true

    log_success "PV 和 StorageClass 已清理"
}

delete_crd() {
    log_info "删除 CRD tenants.rustfs.com..."

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
        log_success "CRD 已删除"
    else
        log_info "CRD 不存在，跳过"
    fi
}

delete_kind_cluster() {
    log_info "删除 Kind 集群 ${CLUSTER_NAME}..."

    if ! command -v kind >/dev/null 2>&1; then
        log_warning "未找到 kind，跳过"
        return 0
    fi

    if kind get clusters 2>/dev/null | grep -q "^${CLUSTER_NAME}$"; then
        kind delete cluster --name ${CLUSTER_NAME}
        log_success "Kind 集群已删除"
    else
        log_info "Kind 集群不存在，跳过"
    fi
}

cleanup_storage_dirs() {
    log_info "清理主机存储目录..."

    for dir in /tmp/rustfs-storage-1 /tmp/rustfs-storage-2 /tmp/rustfs-storage-3; do
        if [ -d "$dir" ]; then
            rm -rf "$dir"
            log_info "已删除 $dir"
        fi
    done

    log_success "存储目录已清理"
}

cleanup_local_files() {
    log_info "清理本地生成文件..."

    if [ -f "deploy/rustfs-operator/crds/tenant-crd.yaml" ]; then
        rm -f deploy/rustfs-operator/crds/tenant-crd.yaml
        log_info "已删除 tenant-crd.yaml"
    fi

    log_success "本地文件已清理"
}

show_next_steps() {
    echo ""
    log_info "重新部署:"
    echo "  ./scripts/deploy/deploy-rustfs-4node.sh"
    echo ""
}

# 解析参数
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
            echo "  -f, --force        跳过确认"
            echo "  -s, --clean-storage 同时删除主机目录 /tmp/rustfs-storage-{1,2,3}"
            echo "  -h, --help         显示帮助"
            exit 0
            ;;
        *)
            log_error "未知参数: $1"
            exit 1
            ;;
    esac
done

trap 'log_error "清理被中断"; exit 1' INT

log_info "=========================================="
log_info "  RustFS 4-node 环境清理"
log_info "=========================================="

confirm_cleanup

echo ""
log_info "开始清理..."
echo ""

# 若集群存在且可连接，先清理 K8s 资源
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
    log_info "Kind 集群 ${CLUSTER_NAME} 不存在，跳过 K8s 资源清理"
fi

cleanup_local_files
delete_kind_cluster

if [ "$CLEAN_STORAGE" = "true" ]; then
    cleanup_storage_dirs
fi

echo ""
show_next_steps

log_success "=========================================="
log_success "  清理完成"
log_success "=========================================="
