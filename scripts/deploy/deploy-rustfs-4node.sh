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
# One-shot deploy: RustFS Operator on a 4-node Kind cluster
#
# Topology: multi-node Kind (1 control-plane + 3 workers) + 4-node Tenant + dual Console
# (Inspired by a similar MinIO multi-node demo layout.)
#
# Steps:
#   - Create Kind cluster (kind-rustfs-cluster.yaml)
#   - Create StorageClass and 12 PersistentVolumes
#   - Deploy RustFS Operator + Operator Console (API + Web)
#   - Deploy 4-node RustFS Tenant
#   - Print access information
#
# Usage:
#   ./scripts/deploy/deploy-rustfs-4node.sh
#
################################################################################

set -e
set -o pipefail

# Always run from project root (script cds here; safe to invoke from any cwd)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$PROJECT_ROOT"

################################################################################
# Colors
################################################################################
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

################################################################################
# Configuration
################################################################################
CLUSTER_NAME="rustfs-cluster"
OPERATOR_NAMESPACE="rustfs-system"
TENANT_NAME="example-tenant"
STORAGE_CLASS="local-storage"
PV_COUNT=12
WORKER_NODES=("${CLUSTER_NAME}-worker" "${CLUSTER_NAME}-worker2" "${CLUSTER_NAME}-worker3")
RUSTFS_RUN_AS_UID=10001

################################################################################
# Logging
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
    log_info "Step $1: $2"
}

################################################################################
# Error handling
################################################################################
trap 'error_handler $? $LINENO' ERR

error_handler() {
    log_error "Script failed at line $2 with exit code $1"
    log_warning "Run ./scripts/cleanup/cleanup-rustfs-4node.sh to tear down"
    exit 1
}

################################################################################
# Dependencies
################################################################################
check_dependencies() {
    log_step "0/12" "Checking required tools"

    local missing_tools=()
    for cmd in kubectl kind docker cargo; do
        if ! command -v $cmd &>/dev/null; then
            missing_tools+=("$cmd")
        fi
    done

    if [ ${#missing_tools[@]} -ne 0 ]; then
        log_error "Missing tools: ${missing_tools[*]}"
        log_info "Install: kubectl, kind, docker, cargo (Rust)"
        exit 1
    fi

    log_success "All required tools are present"
}

################################################################################
# Fix inotify limits (common with Kind multi-node)
################################################################################
fix_inotify_limits() {
    if sudo sysctl -w fs.inotify.max_user_watches=524288 >/dev/null 2>&1 \
        && sudo sysctl -w fs.inotify.max_user_instances=512 >/dev/null 2>&1; then
        log_info "Applied inotify limits"
    else
        log_warning "Could not set inotify limits (may need root). If you see 'too many open files':"
        echo "  sudo sysctl fs.inotify.max_user_watches=524288"
        echo "  sudo sysctl fs.inotify.max_user_instances=512"
    fi
}

################################################################################
# Kind cluster
################################################################################
create_kind_cluster() {
    log_step "1/12" "Creating Kind cluster"

    fix_inotify_limits

    if kind get clusters 2>/dev/null | grep -q "^${CLUSTER_NAME}$"; then
        log_warning "Cluster ${CLUSTER_NAME} already exists"
        read -p "Delete and recreate? (y/n) " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            log_info "Deleting existing cluster..."
            kind delete cluster --name ${CLUSTER_NAME}
            log_success "Existing cluster removed"
        else
            log_info "Using existing cluster"
            kubectl config use-context kind-${CLUSTER_NAME} >/dev/null
            return 0
        fi
    fi

    local kind_config="${PROJECT_ROOT}/deploy/kind/kind-rustfs-cluster.yaml"
    if [ ! -f "$kind_config" ]; then
        log_error "Config file not found: $kind_config"
        exit 1
    fi

    log_info "Creating cluster (1 control-plane + 3 workers; may take a few minutes)..."
    kind create cluster --config "$kind_config"

    kubectl config use-context kind-${CLUSTER_NAME} >/dev/null
    log_success "Kind cluster created"
}

################################################################################
# Wait for nodes
################################################################################
wait_cluster_ready() {
    log_step "2/12" "Waiting for nodes to be Ready"

    log_info "Waiting for all nodes (timeout 5m)..."
    kubectl wait --for=condition=Ready nodes --all --timeout=300s

    # Optional: allow scheduling on control-plane (4 pods can run on 3 workers)
    kubectl taint nodes ${CLUSTER_NAME}-control-plane node-role.kubernetes.io/control-plane:NoSchedule- 2>/dev/null || true

    log_success "All nodes are Ready"
    kubectl get nodes -o wide
}

################################################################################
# Storage dirs on host
################################################################################
create_storage_dirs() {
    log_step "3/12" "Creating local storage directories"

    mkdir -p /tmp/rustfs-storage-{1,2,3}
    log_success "Local storage directories created"
}

################################################################################
# StorageClass
################################################################################
create_storage_class() {
    log_step "4/12" "Creating StorageClass"

    cat <<EOF | kubectl apply -f -
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: ${STORAGE_CLASS}
provisioner: kubernetes.io/no-provisioner
volumeBindingMode: WaitForFirstConsumer
EOF

    log_success "StorageClass created"
}

################################################################################
# PersistentVolumes
################################################################################
create_persistent_volumes() {
    log_step "5/12" "Creating PersistentVolumes"

    log_info "Creating ${PV_COUNT} PersistentVolumes..."

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

    log_success "${PV_COUNT} PersistentVolumes created"
    kubectl get pv
}

################################################################################
# Volume directories inside worker nodes
################################################################################
create_volume_dirs_in_nodes() {
    log_step "6/12" "Creating volume directories on workers"

    for node in "${WORKER_NODES[@]}"; do
        log_info "Creating volume dirs on node ${node}..."
        for i in $(seq 1 ${PV_COUNT}); do
            docker exec ${node} mkdir -p /mnt/data/vol${i} 2>/dev/null || true
            docker exec ${node} chown -R ${RUSTFS_RUN_AS_UID}:${RUSTFS_RUN_AS_UID} /mnt/data/vol${i} 2>/dev/null || true
        done
    done

    log_success "Volume directories created with permissions"
}

################################################################################
# CRD
################################################################################
deploy_crd() {
    log_step "7/12" "Deploying Tenant CRD"

    local crd_dir="deploy/rustfs-operator/crds"
    local crd_file="${crd_dir}/tenant-crd.yaml"
    mkdir -p "$crd_dir"

    log_info "Generating CRD..."
    cargo run --release -- crd -f "$crd_file"

    log_info "Applying CRD..."
    kubectl apply -f "$crd_file"

    log_info "Waiting for CRD to be established..."
    kubectl wait --for condition=established --timeout=60s crd/tenants.rustfs.com

    log_success "CRD deployed"
}

################################################################################
# Namespace
################################################################################
create_namespace() {
    log_step "8/12" "Creating namespace"

    if kubectl get namespace ${OPERATOR_NAMESPACE} &>/dev/null; then
        log_warning "Namespace ${OPERATOR_NAMESPACE} already exists"
    else
        kubectl create namespace ${OPERATOR_NAMESPACE}
        log_success "Namespace created"
    fi
}

################################################################################
# Build and deploy Operator + Console
################################################################################
deploy_operator_and_console() {
    log_step "9/12" "Building and deploying Operator + Console"

    local image_name="rustfs/operator:dev"
    local console_web_image="rustfs/console-web:dev"

    log_info "Building Operator (release)..."
    cargo build --release

    log_info "Building Operator container image..."
    docker build --network=host --no-cache -t "$image_name" . || {
        log_error "Operator image build failed"
        exit 1
    }

    log_info "Building Console Web image..."
    docker build --network=host --no-cache \
        -t "$console_web_image" \
        -f console-web/Dockerfile \
        console-web/ || {
        log_error "Console Web image build failed"
        exit 1
    }

    log_info "Loading images into Kind..."
    kind load docker-image "$image_name" --name ${CLUSTER_NAME} || {
        log_error "Failed to load Operator image into Kind"
        exit 1
    }
    kind load docker-image "$console_web_image" --name ${CLUSTER_NAME} || {
        log_error "Failed to load Console Web image into Kind"
        exit 1
    }

    # Load RustFS server image if present locally
    if docker images --format '{{.Repository}}:{{.Tag}}' | grep -q '^rustfs/rustfs:latest$'; then
        log_info "Loading RustFS server image..."
        kind load docker-image rustfs/rustfs:latest --name ${CLUSTER_NAME} 2>/dev/null || log_warning "Failed to load rustfs/rustfs:latest; Tenant may pull from registry"
    else
        log_warning "rustfs/rustfs:latest not found locally; Tenant will try to pull from registry"
    fi

    log_info "Creating Console JWT Secret..."
    local jwt_secret
    jwt_secret=$(openssl rand -base64 32 2>/dev/null || head -c 32 /dev/urandom | base64)
    kubectl create secret generic rustfs-operator-console-secret \
        --namespace ${OPERATOR_NAMESPACE} \
        --from-literal=jwt-secret="$jwt_secret" \
        --dry-run=client -o yaml | kubectl apply -f -

    log_info "Deploying Operator, Console API, Console Web..."
    kubectl apply -f deploy/k8s-dev/operator-rbac.yaml
    kubectl apply -f deploy/k8s-dev/console-rbac.yaml
    kubectl apply -f deploy/k8s-dev/operator-deployment.yaml
    kubectl apply -f deploy/k8s-dev/console-deployment.yaml
    kubectl apply -f deploy/k8s-dev/console-service.yaml
    kubectl apply -f deploy/k8s-dev/console-frontend-deployment.yaml
    kubectl apply -f deploy/k8s-dev/console-frontend-service.yaml

    log_info "Waiting for Operator (timeout 5m)..."
    kubectl wait --for=condition=available --timeout=300s \
        deployment/rustfs-operator -n ${OPERATOR_NAMESPACE}

    log_info "Waiting for Operator Console..."
    kubectl wait --for=condition=available --timeout=300s \
        deployment/rustfs-operator-console -n ${OPERATOR_NAMESPACE}

    log_info "Waiting for Console Web..."
    kubectl wait --for=condition=available --timeout=300s \
        deployment/rustfs-operator-console-frontend -n ${OPERATOR_NAMESPACE}

    log_success "Operator and Console deployed"
    kubectl get pods -n ${OPERATOR_NAMESPACE}
}

################################################################################
# Tenant (4 nodes)
################################################################################
deploy_tenant() {
    log_step "10/12" "Deploying RustFS Tenant (4 nodes)"

    if [ ! -f "examples/tenant-4nodes.yaml" ]; then
        log_error "File not found: examples/tenant-4nodes.yaml"
        exit 1
    fi

    kubectl apply -f examples/tenant-4nodes.yaml

    log_success "Tenant applied"

    log_info "Waiting for Tenant pods (may take a few minutes)..."
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
            log_success "Tenant pods running ($ready_pods/$expected_pods Running)"
            break
        fi

        log_info "Waiting for pods... ($ready_pods/$expected_pods ready)"
        sleep 5
        attempt=$((attempt + 1))
    done

    if [ "$ready_pods" -lt "$expected_pods" ]; then
        log_warning "Some pods may still be starting ($ready_pods/$expected_pods)"
    fi

    kubectl get pods -n ${OPERATOR_NAMESPACE} -l rustfs.tenant=${TENANT_NAME}
    kubectl get pvc -n ${OPERATOR_NAMESPACE}
}

################################################################################
# Access info
################################################################################
get_access_info() {
    log_step "11/12" "Gathering access information"

    log_info "Fetching Operator Console token..."
    if kubectl get secret rustfs-operator-console-secret -n ${OPERATOR_NAMESPACE} &>/dev/null; then
        OPERATOR_TOKEN=$(kubectl create token rustfs-operator -n ${OPERATOR_NAMESPACE} --duration=24h 2>/dev/null || echo "")
        if [ -n "$OPERATOR_TOKEN" ]; then
            echo "$OPERATOR_TOKEN" > /tmp/rustfs-operator-console-token.txt
            log_success "Token saved to /tmp/rustfs-operator-console-token.txt"
        fi
    fi

    if kubectl get tenant ${TENANT_NAME} -n ${OPERATOR_NAMESPACE} &>/dev/null; then
        TENANT_STATE=$(kubectl get tenant ${TENANT_NAME} -n ${OPERATOR_NAMESPACE} \
            -o jsonpath='{.status.currentState}' 2>/dev/null || echo "Unknown")
        log_info "Tenant status: ${TENANT_STATE}"
    fi
}

################################################################################
# Summary
################################################################################
show_summary() {
    log_step "12/12" "Deployment summary"

    log_header "Deployment complete"

    echo ""
    echo -e "${BLUE}📊 Cluster${NC}"
    echo "  Name: ${CLUSTER_NAME}"
    echo "  Nodes: 4 (1 control-plane + 3 workers)"
    echo ""

    echo -e "${BLUE}📦 Deployed${NC}"
    echo "  Operator + Console API + Console Web"
    echo "  Tenant: ${TENANT_NAME} (4 servers, 2 volumes each)"
    echo ""

    echo -e "${GREEN}======================================${NC}"
    echo -e "${GREEN}🚀 Access${NC}"
    echo -e "${GREEN}======================================${NC}"
    echo ""

    echo -e "${YELLOW}1. Operator Console Web (manage Tenants)${NC}"
    echo "   Use: create / delete / manage Tenants"
    echo -e "   URL: ${CYAN}http://localhost:8080${NC}"
    echo "   Auth: Kubernetes token (see below)"
    echo ""
    echo "   Port-forward:"
    echo -e "   ${BLUE}kubectl port-forward svc/rustfs-operator-console-frontend -n ${OPERATOR_NAMESPACE} 8080:80${NC}"
    echo ""
    echo "   Get token:"
    echo -e "   ${BLUE}kubectl create token rustfs-operator -n ${OPERATOR_NAMESPACE} --duration=24h${NC}"
    echo ""

    echo -e "${YELLOW}2. Tenant Console (RustFS UI)${NC}"
    echo "   Use: upload/download, buckets"
    echo -e "   URL: ${CYAN}http://localhost:9001${NC}"
    echo -e "   Username: ${GREEN}admin123${NC}"
    echo -e "   Password: ${GREEN}admin12345${NC}"
    echo ""
    echo "   Port-forward:"
    echo -e "   ${BLUE}kubectl port-forward svc/${TENANT_NAME}-console -n ${OPERATOR_NAMESPACE} 9001:9001${NC}"
    echo ""

    echo -e "${YELLOW}3. RustFS S3 API${NC}"
    echo -e "   URL: ${CYAN}http://localhost:9000${NC}"
    echo -e "   Access Key: ${GREEN}admin123${NC}"
    echo -e "   Secret Key: ${GREEN}admin12345${NC}"
    echo ""
    echo "   Port-forward:"
    echo -e "   ${BLUE}kubectl port-forward svc/${TENANT_NAME}-io -n ${OPERATOR_NAMESPACE} 9000:9000${NC}"
    echo ""

    echo -e "${GREEN}======================================${NC}"
    echo -e "${GREEN}📝 Useful commands${NC}"
    echo -e "${GREEN}======================================${NC}"
    echo ""
    echo "Resources:"
    echo -e "  ${BLUE}kubectl get all -n ${OPERATOR_NAMESPACE}${NC}"
    echo -e "  ${BLUE}kubectl get tenant -n ${OPERATOR_NAMESPACE}${NC}"
    echo ""
    echo "Logs:"
    echo -e "  ${BLUE}kubectl logs -f deployment/rustfs-operator -n ${OPERATOR_NAMESPACE}${NC}"
    echo -e "  ${BLUE}kubectl logs -f ${TENANT_NAME}-primary-0 -n ${OPERATOR_NAMESPACE}${NC}"
    echo ""
    echo "Tear down:"
    echo -e "  ${RED}./scripts/cleanup/cleanup-rustfs-4node.sh${NC}"
    echo ""

    log_success "Done. Use Operator Console and Tenant Console as above."
    echo ""
}

################################################################################
# Main
################################################################################
main() {
    log_header "RustFS Operator 4-node deploy"
    log_info "Topology: Kind multi-node + 4-node Tenant + dual Console"
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

case "${1:-}" in
    -h|--help)
        echo "Usage: $0"
        echo ""
        echo "RustFS Operator 4-node demo (Kind multi-node + 4-node Tenant + dual Console)"
        echo ""
        echo "Requires: kubectl, kind, docker, cargo (Rust)"
        echo ""
        echo "Cleanup: ./scripts/cleanup/cleanup-rustfs-4node.sh"
        exit 0
        ;;
esac

main "$@"
