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
# RustFS Operator 4-node 一键部署脚本
#
# 架构: Kind 多节点 (1 control-plane + 3 workers) + 4 节点 Tenant + 双 Console
# 与 MinIO deploy-minio-v5.sh 架构一致
#
# 功能:
#   - 创建 Kind 集群 (kind-rustfs-cluster.yaml)
#   - 创建 StorageClass 和 12 个 PersistentVolumes
#   - 部署 RustFS Operator + Operator Console (API + Web)
#   - 部署 4 节点 RustFS Tenant
#   - 获取并显示访问信息
#
# 使用:
#   ./deploy-rustfs-4node.sh
#
################################################################################

set -e
set -o pipefail

# 保证从项目根目录执行（可从任意位置调用本脚本）
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$PROJECT_ROOT"

################################################################################
# 颜色定义
################################################################################
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

################################################################################
# 配置变量
################################################################################
CLUSTER_NAME="rustfs-cluster"
OPERATOR_NAMESPACE="rustfs-system"
TENANT_NAME="example-tenant"
STORAGE_CLASS="local-storage"
PV_COUNT=12
WORKER_NODES=("${CLUSTER_NAME}-worker" "${CLUSTER_NAME}-worker2" "${CLUSTER_NAME}-worker3")
RUSTFS_RUN_AS_UID=10001

################################################################################
# 日志函数
################################################################################
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

log_header() {
    echo ""
    echo -e "${CYAN}======================================${NC}"
    echo -e "${CYAN}$1${NC}"
    echo -e "${CYAN}======================================${NC}"
}

log_step() {
    echo ""
    log_info "步骤 $1: $2"
}

################################################################################
# 错误处理
################################################################################
trap 'error_handler $? $LINENO' ERR

error_handler() {
    log_error "脚本在第 $2 行失败，退出码: $1"
    log_warning "部署失败，可运行 ./scripts/cleanup/cleanup-rustfs-4node.sh 清理环境"
    exit 1
}

################################################################################
# 检查依赖工具
################################################################################
check_dependencies() {
    log_step "0/12" "检查必要工具"

    local missing_tools=()
    for cmd in kubectl kind docker cargo; do
        if ! command -v $cmd &>/dev/null; then
            missing_tools+=("$cmd")
        fi
    done

    if [ ${#missing_tools[@]} -ne 0 ]; then
        log_error "缺少必要工具: ${missing_tools[*]}"
        log_info "请先安装: kubectl, kind, docker, cargo (Rust)"
        exit 1
    fi

    log_success "所有必要工具已安装"
}

################################################################################
# 修复 inotify 限制 (Kind 多节点常见问题)
################################################################################
fix_inotify_limits() {
    if sudo sysctl -w fs.inotify.max_user_watches=524288 >/dev/null 2>&1 \
        && sudo sysctl -w fs.inotify.max_user_instances=512 >/dev/null 2>&1; then
        log_info "已应用 inotify 限制"
    else
        log_warning "无法设置 inotify 限制 (可能需要 root)。若出现 'too many open files' 错误:"
        echo "  sudo sysctl fs.inotify.max_user_watches=524288"
        echo "  sudo sysctl fs.inotify.max_user_instances=512"
    fi
}

################################################################################
# 创建 Kind 集群
################################################################################
create_kind_cluster() {
    log_step "1/12" "创建 Kind 集群"

    fix_inotify_limits

    if kind get clusters 2>/dev/null | grep -q "^${CLUSTER_NAME}$"; then
        log_warning "集群 ${CLUSTER_NAME} 已存在"
        read -p "是否删除并重建? (y/n) " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            log_info "删除现有集群..."
            kind delete cluster --name ${CLUSTER_NAME}
            log_success "现有集群已删除"
        else
            log_info "使用现有集群"
            kubectl config use-context kind-${CLUSTER_NAME} >/dev/null
            return 0
        fi
    fi

    local kind_config="${PROJECT_ROOT}/deploy/kind/kind-rustfs-cluster.yaml"
    if [ ! -f "$kind_config" ]; then
        log_error "配置文件不存在: $kind_config"
        exit 1
    fi

    log_info "创建新集群 (1 control-plane + 3 workers，约需几分钟)..."
    kind create cluster --config "$kind_config"

    kubectl config use-context kind-${CLUSTER_NAME} >/dev/null
    log_success "Kind 集群已创建"
}

################################################################################
# 等待集群就绪
################################################################################
wait_cluster_ready() {
    log_step "2/12" "等待集群节点就绪"

    log_info "等待所有节点就绪 (超时 5 分钟)..."
    kubectl wait --for=condition=Ready nodes --all --timeout=300s

    # 允许在 control-plane 上调度 (可选，4 个 Pod 分布在 3 个 worker 上即可)
    kubectl taint nodes ${CLUSTER_NAME}-control-plane node-role.kubernetes.io/control-plane:NoSchedule- 2>/dev/null || true

    log_success "所有节点已就绪"
    kubectl get nodes -o wide
}

################################################################################
# 创建存储目录
################################################################################
create_storage_dirs() {
    log_step "3/12" "创建本地存储目录"

    mkdir -p /tmp/rustfs-storage-{1,2,3}
    log_success "本地存储目录已创建"
}

################################################################################
# 创建 StorageClass
################################################################################
create_storage_class() {
    log_step "4/12" "创建 StorageClass"

    cat <<EOF | kubectl apply -f -
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: ${STORAGE_CLASS}
provisioner: kubernetes.io/no-provisioner
volumeBindingMode: WaitForFirstConsumer
EOF

    log_success "StorageClass 已创建"
}

################################################################################
# 创建 PersistentVolumes
################################################################################
create_persistent_volumes() {
    log_step "5/12" "创建 PersistentVolumes"

    log_info "创建 ${PV_COUNT} 个 PersistentVolumes..."

    for i in $(seq 1 ${PV_COUNT}); do
        worker_num=$(( (i-1) % 3 + 1 ))
        cat <<EOF | kubectl apply -f -
apiVersion: v1
kind: PersistentVolume
metadata:
  name: rustfs-pv-${i}
spec:
  capacity:
    storage: 10Gi
  volumeMode: Filesystem
  accessModes:
  - ReadWriteOnce
  persistentVolumeReclaimPolicy: Retain
  storageClassName: ${STORAGE_CLASS}
  local:
    path: /mnt/data/vol${i}
  nodeAffinity:
    required:
      nodeSelectorTerms:
      - matchExpressions:
        - key: worker-group
          operator: In
          values:
          - storage-${worker_num}
EOF
    done

    log_success "${PV_COUNT} 个 PersistentVolumes 已创建"
    kubectl get pv
}

################################################################################
# 在 Worker 节点中创建卷目录
################################################################################
create_volume_dirs_in_nodes() {
    log_step "6/12" "在 Worker 节点中创建卷目录"

    for node in "${WORKER_NODES[@]}"; do
        log_info "在节点 ${node} 创建卷目录..."
        for i in $(seq 1 ${PV_COUNT}); do
            docker exec ${node} mkdir -p /mnt/data/vol${i} 2>/dev/null || true
            docker exec ${node} chown -R ${RUSTFS_RUN_AS_UID}:${RUSTFS_RUN_AS_UID} /mnt/data/vol${i} 2>/dev/null || true
        done
    done

    log_success "所有卷目录已创建并设置权限"
}

################################################################################
# 生成并部署 CRD
################################################################################
deploy_crd() {
    log_step "7/12" "部署 Tenant CRD"

    local crd_dir="deploy/rustfs-operator/crds"
    local crd_file="${crd_dir}/tenant-crd.yaml"
    mkdir -p "$crd_dir"

    log_info "生成 CRD..."
    cargo run --release -- crd -f "$crd_file"

    log_info "应用 CRD..."
    kubectl apply -f "$crd_file"

    log_info "等待 CRD 就绪..."
    kubectl wait --for condition=established --timeout=60s crd/tenants.rustfs.com

    log_success "CRD 已部署"
}

################################################################################
# 创建命名空间
################################################################################
create_namespace() {
    log_step "8/12" "创建命名空间"

    if kubectl get namespace ${OPERATOR_NAMESPACE} &>/dev/null; then
        log_warning "命名空间 ${OPERATOR_NAMESPACE} 已存在"
    else
        kubectl create namespace ${OPERATOR_NAMESPACE}
        log_success "命名空间已创建"
    fi
}

################################################################################
# 构建并部署 Operator + Console
################################################################################
deploy_operator_and_console() {
    log_step "9/12" "构建并部署 Operator + Console"

    local image_name="rustfs/operator:dev"
    local console_web_image="rustfs/console-web:dev"

    log_info "构建 Operator (release)..."
    cargo build --release

    log_info "构建 Operator Docker 镜像..."
    docker build --network=host --no-cache -t "$image_name" . || {
        log_error "Operator 镜像构建失败"
        exit 1
    }

    log_info "构建 Console Web 镜像..."
    docker build --network=host --no-cache \
        -t "$console_web_image" \
        -f console-web/Dockerfile \
        console-web/ || {
        log_error "Console Web 镜像构建失败"
        exit 1
    }

    log_info "加载镜像到 Kind 集群..."
    kind load docker-image "$image_name" --name ${CLUSTER_NAME} || {
        log_error "加载 Operator 镜像失败"
        exit 1
    }
    kind load docker-image "$console_web_image" --name ${CLUSTER_NAME} || {
        log_error "加载 Console Web 镜像失败"
        exit 1
    }

    # 若存在 rustfs 服务端镜像，一并加载
    if docker images --format '{{.Repository}}:{{.Tag}}' | grep -q '^rustfs/rustfs:latest$'; then
        log_info "加载 RustFS 服务端镜像..."
        kind load docker-image rustfs/rustfs:latest --name ${CLUSTER_NAME} 2>/dev/null || log_warning "rustfs/rustfs:latest 加载失败，Tenant 可能需从 registry 拉取"
    else
        log_warning "未找到 rustfs/rustfs:latest 本地镜像，Tenant 将尝试从 registry 拉取"
    fi

    log_info "创建 Console JWT Secret..."
    local jwt_secret
    jwt_secret=$(openssl rand -base64 32 2>/dev/null || head -c 32 /dev/urandom | base64)
    kubectl create secret generic rustfs-operator-console-secret \
        --namespace ${OPERATOR_NAMESPACE} \
        --from-literal=jwt-secret="$jwt_secret" \
        --dry-run=client -o yaml | kubectl apply -f -

    log_info "部署 Operator、Console API、Console Web..."
    kubectl apply -f deploy/k8s-dev/operator-rbac.yaml
    kubectl apply -f deploy/k8s-dev/console-rbac.yaml
    kubectl apply -f deploy/k8s-dev/operator-deployment.yaml
    kubectl apply -f deploy/k8s-dev/console-deployment.yaml
    kubectl apply -f deploy/k8s-dev/console-service.yaml
    kubectl apply -f deploy/k8s-dev/console-frontend-deployment.yaml
    kubectl apply -f deploy/k8s-dev/console-frontend-service.yaml

    log_info "等待 Operator 就绪 (超时 5 分钟)..."
    kubectl wait --for=condition=available --timeout=300s \
        deployment/rustfs-operator -n ${OPERATOR_NAMESPACE}

    log_info "等待 Operator Console 就绪..."
    kubectl wait --for=condition=available --timeout=300s \
        deployment/rustfs-operator-console -n ${OPERATOR_NAMESPACE}

    log_info "等待 Console Web 就绪..."
    kubectl wait --for=condition=available --timeout=300s \
        deployment/rustfs-operator-console-frontend -n ${OPERATOR_NAMESPACE}

    log_success "Operator 和 Console 已部署"
    kubectl get pods -n ${OPERATOR_NAMESPACE}
}

################################################################################
# 部署 Tenant (4 节点)
################################################################################
deploy_tenant() {
    log_step "10/12" "部署 RustFS Tenant (4 节点)"

    if [ ! -f "examples/tenant-4nodes.yaml" ]; then
        log_error "配置文件 examples/tenant-4nodes.yaml 不存在"
        exit 1
    fi

    kubectl apply -f examples/tenant-4nodes.yaml

    log_success "Tenant 已提交"

    log_info "等待 Tenant Pods 启动 (约需几分钟)..."
    sleep 15

    local max_attempts=60
    local attempt=0
    local expected_pods=4
    local ready_pods=0

    while [ $attempt -lt $max_attempts ]; do
        local ready_pods
        ready_pods=$(kubectl get pods -n ${OPERATOR_NAMESPACE} \
            -l rustfs.tenant=${TENANT_NAME} \
            --field-selector=status.phase=Running \
            --no-headers 2>/dev/null | wc -l | tr -d ' ')

        if [ "$ready_pods" -ge "$expected_pods" ]; then
            log_success "Tenant Pods 已启动 ($ready_pods/$expected_pods Running)"
            break
        fi

        log_info "等待 Pods 启动... ($ready_pods/$expected_pods ready)"
        sleep 5
        attempt=$((attempt + 1))
    done

    if [ "$ready_pods" -lt "$expected_pods" ]; then
        log_warning "部分 Pods 可能还在启动中 ($ready_pods/$expected_pods)"
    fi

    kubectl get pods -n ${OPERATOR_NAMESPACE} -l rustfs.tenant=${TENANT_NAME}
    kubectl get pvc -n ${OPERATOR_NAMESPACE}
}

################################################################################
# 获取访问信息
################################################################################
get_access_info() {
    log_step "11/12" "获取访问信息"

    # Operator Console Token
    log_info "获取 Operator Console Token..."
    if kubectl get secret rustfs-operator-console-secret -n ${OPERATOR_NAMESPACE} &>/dev/null; then
        OPERATOR_TOKEN=$(kubectl create token rustfs-operator -n ${OPERATOR_NAMESPACE} --duration=24h 2>/dev/null || echo "")
        if [ -n "$OPERATOR_TOKEN" ]; then
            echo "$OPERATOR_TOKEN" > /tmp/rustfs-operator-console-token.txt
            log_success "Token 已保存到 /tmp/rustfs-operator-console-token.txt"
        fi
    fi

    # Tenant 状态
    if kubectl get tenant ${TENANT_NAME} -n ${OPERATOR_NAMESPACE} &>/dev/null; then
        TENANT_STATE=$(kubectl get tenant ${TENANT_NAME} -n ${OPERATOR_NAMESPACE} \
            -o jsonpath='{.status.currentState}' 2>/dev/null || echo "Unknown")
        log_info "Tenant 状态: ${TENANT_STATE}"
    fi
}

################################################################################
# 显示部署摘要
################################################################################
show_summary() {
    log_step "12/12" "部署摘要"

    log_header "部署完成"

    echo ""
    echo -e "${BLUE}📊 集群信息${NC}"
    echo "  集群名称: ${CLUSTER_NAME}"
    echo "  节点数量: 4 (1 control-plane + 3 workers)"
    echo ""

    echo -e "${BLUE}📦 已部署组件${NC}"
    echo "  Operator + Console API + Console Web"
    echo "  Tenant: ${TENANT_NAME} (4 servers, 2 volumes each)"
    echo ""

    echo -e "${GREEN}======================================${NC}"
    echo -e "${GREEN}🚀 访问信息${NC}"
    echo -e "${GREEN}======================================${NC}"
    echo ""

    echo -e "${YELLOW}1. Operator Console Web (管理 Tenant)${NC}"
    echo "   用途: 创建/删除/管理 Tenant"
    echo -e "   访问: ${CYAN}http://localhost:8080${NC}"
    echo "   认证: K8s Token (见下方)"
    echo ""
    echo "   启动端口转发:"
    echo -e "   ${BLUE}kubectl port-forward svc/rustfs-operator-console-frontend -n ${OPERATOR_NAMESPACE} 8080:80${NC}"
    echo ""
    echo "   获取 Token:"
    echo -e "   ${BLUE}kubectl create token rustfs-operator -n ${OPERATOR_NAMESPACE} --duration=24h${NC}"
    echo ""

    echo -e "${YELLOW}2. Tenant Console (管理数据)${NC}"
    echo "   用途: 上传/下载文件，管理 Buckets"
    echo -e "   访问: ${CYAN}http://localhost:9001${NC}"
    echo -e "   用户名: ${GREEN}admin123${NC}"
    echo -e "   密码: ${GREEN}admin12345${NC}"
    echo ""
    echo "   启动端口转发:"
    echo -e "   ${BLUE}kubectl port-forward svc/${TENANT_NAME}-console -n ${OPERATOR_NAMESPACE} 9001:9001${NC}"
    echo ""

    echo -e "${YELLOW}3. RustFS S3 API${NC}"
    echo -e "   访问: ${CYAN}http://localhost:9000${NC}"
    echo -e "   Access Key: ${GREEN}admin123${NC}"
    echo -e "   Secret Key: ${GREEN}admin12345${NC}"
    echo ""
    echo "   启动端口转发:"
    echo -e "   ${BLUE}kubectl port-forward svc/${TENANT_NAME}-io -n ${OPERATOR_NAMESPACE} 9000:9000${NC}"
    echo ""

    echo -e "${GREEN}======================================${NC}"
    echo -e "${GREEN}📝 常用命令${NC}"
    echo -e "${GREEN}======================================${NC}"
    echo ""
    echo "查看资源:"
    echo -e "  ${BLUE}kubectl get all -n ${OPERATOR_NAMESPACE}${NC}"
    echo -e "  ${BLUE}kubectl get tenant -n ${OPERATOR_NAMESPACE}${NC}"
    echo ""
    echo "查看日志:"
    echo -e "  ${BLUE}kubectl logs -f deployment/rustfs-operator -n ${OPERATOR_NAMESPACE}${NC}"
    echo -e "  ${BLUE}kubectl logs -f ${TENANT_NAME}-primary-0 -n ${OPERATOR_NAMESPACE}${NC}"
    echo ""
    echo "销毁环境:"
    echo -e "  ${RED}./scripts/cleanup/cleanup-rustfs-4node.sh${NC}"
    echo ""

    log_success "部署完成，可访问 Operator Console 和 Tenant Console"
    echo ""
}

################################################################################
# 主函数
################################################################################
main() {
    log_header "RustFS Operator 4-node 一键部署"
    log_info "架构: Kind 多节点 + 4 节点 Tenant + 双 Console"
    echo ""

    check_dependencies
    create_kind_cluster
    wait_cluster_ready
    create_storage_dirs
    create_storage_class
    create_persistent_volumes
    create_volume_dirs_in_nodes
    deploy_crd
    create_namespace
    deploy_operator_and_console
    deploy_tenant
    get_access_info
    show_summary
}

# 解析参数
case "${1:-}" in
    -h|--help)
        echo "Usage: $0"
        echo ""
        echo "RustFS Operator 4-node 一键部署 (Kind 多节点 + 4 节点 Tenant + 双 Console)"
        echo ""
        echo "依赖: kubectl, kind, docker, cargo (Rust)"
        echo ""
        echo "清理: ./scripts/cleanup/cleanup-rustfs-4node.sh"
        exit 0
        ;;
esac

main "$@"
