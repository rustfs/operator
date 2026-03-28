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

# RustFS Operator deployment script - uses examples/simple-tenant.yaml
# Deploys Operator, Console (API) and Console Web (frontend) as Kubernetes Deployments (Pods in K8s)
# Images built locally and loaded into kind. For quick deployment and CRD modification verification.

set -e

# Always run from project root (script cds here; safe to invoke from any cwd)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$PROJECT_ROOT"

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

# Check required tools
check_prerequisites() {
    log_info "Checking required tools..."

    local missing_tools=()

    command -v kubectl >/dev/null 2>&1 || missing_tools+=("kubectl")
    command -v cargo >/dev/null 2>&1 || missing_tools+=("cargo")
    command -v kind >/dev/null 2>&1 || missing_tools+=("kind")
    command -v docker >/dev/null 2>&1 || missing_tools+=("docker")

    if [ ${#missing_tools[@]} -ne 0 ]; then
        log_error "Missing required tools: ${missing_tools[*]}"
        exit 1
    fi

    log_success "All required tools are installed"
}

# Fix "too many open files" for kind (inotify limits)
# See: https://kind.sigs.k8s.io/docs/user/known-issues/#pod-errors-due-to-too-many-open-files
fix_inotify_limits() {
    log_info "Applying inotify limits (fix for 'too many open files')..."

    local sysctl_conf="/etc/sysctl.d/99-rustfs-kind.conf"
    local persisted=false

    if sudo sysctl -w fs.inotify.max_user_watches=524288 >/dev/null 2>&1 \
        && sudo sysctl -w fs.inotify.max_user_instances=512 >/dev/null 2>&1; then
        log_success "Inotify limits applied (current session)"
        persisted=true
    fi

    if sudo test -w /etc/sysctl.d 2>/dev/null; then
        if ! sudo grep -qs "fs.inotify.max_user_watches" "$sysctl_conf" 2>/dev/null; then
            printf 'fs.inotify.max_user_watches = 524288\nfs.inotify.max_user_instances = 512\n' \
                | sudo tee "$sysctl_conf" >/dev/null 2>&1 && \
                log_success "Inotify limits persisted to $sysctl_conf"
        fi
    fi

    if [ "$persisted" = true ]; then
        return 0
    fi

    log_warning "Could not set inotify limits (may need root). If you see kube-proxy 'too many open files' errors:"
    echo "  sudo sysctl fs.inotify.max_user_watches=524288"
    echo "  sudo sysctl fs.inotify.max_user_instances=512"
    echo "  # Make persistent: add to /etc/sysctl.conf or $sysctl_conf"
    return 1
}

# Check Kubernetes cluster connection
check_cluster() {
    log_info "Checking Kubernetes cluster connection..."

    if ! kubectl cluster-info >/dev/null 2>&1; then
        log_error "Unable to connect to Kubernetes cluster"
        log_info "Attempting to start kind cluster..."

        fix_inotify_limits || true

        if kind get clusters | grep -q "rustfs-dev"; then
            log_info "Detected kind cluster 'rustfs-dev', attempting to restart..."
            kind delete cluster --name rustfs-dev
        fi

        log_info "Creating new kind cluster..."
        kind create cluster --name rustfs-dev
    else
        fix_inotify_limits || true
    fi

    log_success "Kubernetes cluster connection OK: $(kubectl config current-context)"
}

# Generate and apply CRD
deploy_crd() {
    log_info "Generating CRD..."

    # Create CRD directory
    local crd_dir="deploy/rustfs-operator/crds"
    local crd_file="${crd_dir}/tenant-crd.yaml"

    mkdir -p "$crd_dir"

    # Generate CRD to specified directory
    cargo run --release -- crd -f "$crd_file"

    log_info "Applying CRD..."
    kubectl apply -f "$crd_file"

    # Wait for CRD to be ready
    log_info "Waiting for CRD to be ready..."
    kubectl wait --for condition=established --timeout=60s crd/tenants.rustfs.com

    log_success "CRD deployed"
}

# Create namespace
create_namespace() {
    log_info "Creating namespace: rustfs-system..."

    if kubectl get namespace rustfs-system >/dev/null 2>&1; then
        log_warning "Namespace rustfs-system already exists"
    else
        kubectl create namespace rustfs-system
        log_success "Namespace created"
    fi
}

# Build operator
build_operator() {
    log_info "Building operator (release mode)..."
    cargo build --release
    log_success "Operator build completed"
}

# Build Console Web (frontend) Docker image. Uses default /api/v1 (nginx in image proxies to backend).
build_console_web_image() {
    log_info "Building Console Web Docker image (default /api/v1, no build-arg)..."

    if ! docker build --network=host --no-cache \
        -t rustfs/console-web:dev \
        -f console-web/Dockerfile \
        console-web/; then
        log_error "Console Web Docker build failed"
        exit 1
    fi

    log_success "Console Web image built: rustfs/console-web:dev"
}

# Build Docker image and deploy Operator + Console + Console Web as Kubernetes Deployments
deploy_operator_and_console() {
    local kind_cluster="rustfs-dev"
    local image_name="rustfs/operator:dev"
    local console_web_image="rustfs/console-web:dev"

    log_info "Building Operator Docker image..."

    # Use host network so build container can reach crates.io when host DNS is used (e.g. systemd-resolved)
    if ! docker build --network=host --no-cache -t "$image_name" .; then
        log_error "Docker build failed"
        exit 1
    fi

    build_console_web_image

    log_info "Loading images into kind cluster '$kind_cluster'..."

    if ! kind load docker-image "$image_name" --name "$kind_cluster"; then
        log_error "Failed to load operator image into kind cluster"
        log_info "Verify: 1) kind cluster exists: kind get clusters"
        log_info "        2) kind cluster 'rustfs-dev' exists: kind get clusters"
        log_info "        3) Docker is running and accessible"
        exit 1
    fi

    if ! kind load docker-image "$console_web_image" --name "$kind_cluster"; then
        log_error "Failed to load console-web image into kind cluster"
        exit 1
    fi

    log_info "Creating Console JWT secret..."

    local jwt_secret
    jwt_secret=$(openssl rand -base64 32 2>/dev/null || head -c 32 /dev/urandom | base64)

    kubectl create secret generic rustfs-operator-console-secret \
        --namespace rustfs-system \
        --from-literal=jwt-secret="$jwt_secret" \
        --dry-run=client -o yaml | kubectl apply -f -

    log_info "Deploying Operator, Console and Console Web (Deployments)..."

    kubectl apply -f deploy/k8s-dev/operator-rbac.yaml
    kubectl apply -f deploy/k8s-dev/console-rbac.yaml
    kubectl apply -f deploy/k8s-dev/operator-deployment.yaml
    kubectl apply -f deploy/k8s-dev/console-deployment.yaml
    kubectl apply -f deploy/k8s-dev/console-service.yaml
    kubectl apply -f deploy/k8s-dev/console-frontend-deployment.yaml
    kubectl apply -f deploy/k8s-dev/console-frontend-service.yaml

    log_success "Operator, Console and Console Web deployed to Kubernetes"
}

# Deploy Tenant (EC 2+1 configuration)
deploy_tenant() {
    log_info "Deploying RustFS Tenant (using examples/simple-tenant.yaml)..."

    kubectl apply -f examples/simple-tenant.yaml

    log_success "Tenant submitted"
}

# Wait for pods to be ready (1 operator + 1 console + 1 console-web + 2 tenant = 5)
wait_for_pods() {
    log_info "Waiting for pods to start (max 5 minutes)..."

    local timeout=300
    local elapsed=0
    local interval=5
    local expected_pods=5

    while [ $elapsed -lt $timeout ]; do
        local pod_list
        pod_list=$(kubectl get pods -n rustfs-system --no-headers 2>/dev/null) || true
        local ready_count=0
        local total_count=0
        if [ -n "$pod_list" ]; then
            ready_count=$(echo "$pod_list" | grep -c "Running" 2>/dev/null) || ready_count=0
            total_count=$(echo "$pod_list" | wc -l)
        fi
        # Ensure integer (strip whitespace/newlines) to avoid "integer expression expected"
        ready_count=$((ready_count + 0))
        total_count=$((total_count + 0))

        if [ "$ready_count" -eq "$expected_pods" ] && [ "$total_count" -eq "$expected_pods" ]; then
            log_success "All pods are ready ($expected_pods/$expected_pods Running)"
            return 0
        fi

        echo -ne "${BLUE}[INFO]${NC} Pod status: $ready_count/$expected_pods Running, waited ${elapsed}s...\r"
        sleep $interval
        elapsed=$((elapsed + interval))
    done

    echo "" # New line
    log_warning "Wait timeout, but continuing..."
    return 1
}

# Show deployment status
show_status() {
    log_info "=========================================="
    log_info "  Deployment Status"
    log_info "=========================================="
    echo ""

    log_info "1. Deployment status:"
    kubectl get deployment -n rustfs-system
    echo ""

    log_info "2. Tenant status:"
    kubectl get tenant -n rustfs-system
    echo ""

    log_info "3. Pod status:"
    kubectl get pods -n rustfs-system -o wide
    echo ""

    log_info "4. Service status:"
    kubectl get svc -n rustfs-system
    echo ""

    log_info "5. PVC status:"
    kubectl get pvc -n rustfs-system
    echo ""

    log_info "6. StatefulSet status:"
    kubectl get statefulset -n rustfs-system
    echo ""
}

# Show access information
show_access_info() {
    log_info "=========================================="
    log_info "  Access Information"
    log_info "=========================================="
    echo ""

    echo "📋 View logs:"
    echo "  Operator:   kubectl logs -f deployment/rustfs-operator -n rustfs-system"
    echo "  Console:    kubectl logs -f deployment/rustfs-operator-console -n rustfs-system"
    echo "  Console UI: kubectl logs -f deployment/rustfs-operator-console-frontend -n rustfs-system"
    echo "  RustFS:     kubectl logs -f example-tenant-primary-0 -n rustfs-system"
    echo ""

    echo "🔌 Port forward S3 API (9000):"
    echo "  kubectl port-forward -n rustfs-system svc/rustfs 9000:9000"
    echo ""

    echo "🌐 Port forward RustFS Web Console (9001):"
    echo "  kubectl port-forward -n rustfs-system svc/example-tenant-console 9001:9001"
    echo ""

    echo "🖥️  Operator Console API (port 9090):"
    echo "  kubectl port-forward -n rustfs-system svc/rustfs-operator-console 9090:9090"
    echo "  Then: curl http://localhost:9090/healthz"
    echo ""

    echo "🖥️  Operator Console Web UI (port 8080):"
    echo "  kubectl port-forward -n rustfs-system svc/rustfs-operator-console-frontend 8080:80"
    echo "  Then open: http://localhost:8080  (API is same-origin /api/v1, nginx in pod proxies to backend)"
    echo "  If login still goes to :9090: clear site localStorage (key rustfs_console_api_base_url) or open in private window"
    echo ""

    echo "🔐 RustFS Credentials:"
    echo "  Username: admin"
    echo "  Password: admin123"
    echo ""

    echo "🔑 Operator Console Login:"
    echo "  Create K8s token: kubectl create token rustfs-operator -n rustfs-system --duration=24h"
    echo "  Login: use the token in Console Web UI at http://localhost:8080 (or POST /api/v1/login when same-origin)"
    echo "  Docs: deploy/README.md、scripts/README.md"
    echo ""

    echo "📊 Check cluster status:"
    echo "  ./scripts/check/check-rustfs.sh"
    echo ""

    echo "🗑️  Cleanup deployment:"
    echo "  ./scripts/cleanup/cleanup-rustfs.sh"
    echo ""

    echo "⚠️  If pods show 'ImagePullBackOff' or 'image not present':"
    echo "  docker build --network=host -t rustfs/operator:dev ."
    echo "  docker build --network=host --no-cache -t rustfs/console-web:dev -f console-web/Dockerfile console-web/"
    echo "  kind load docker-image rustfs/operator:dev --name rustfs-dev"
    echo "  kind load docker-image rustfs/console-web:dev --name rustfs-dev"
    echo "  kubectl rollout restart deployment -n rustfs-system"
    echo ""
}

# Main flow
main() {
    log_info "=========================================="
    log_info "  RustFS Operator Deployment Script"
    log_info "  Using: examples/simple-tenant.yaml"
    log_info "=========================================="
    echo ""

    check_prerequisites
    check_cluster

    log_info "Starting deployment..."
    echo ""

    deploy_crd
    create_namespace
    build_operator
    deploy_operator_and_console
    deploy_tenant

    echo ""
    wait_for_pods

    echo ""
    show_status
    show_access_info

    log_success "=========================================="
    log_success "  Deployment completed!"
    log_success "=========================================="
}

# Catch Ctrl+C
trap 'log_error "Deployment interrupted"; exit 1' INT

# Parse arguments
case "${1:-}" in
    --fix-limits)
        log_info "Fix inotify limits for kind (kube-proxy 'too many open files')"
        fix_inotify_limits
        echo ""
        log_info "If cluster already has issues, delete and recreate:"
        echo "  kind delete cluster --name rustfs-dev"
        echo "  ./scripts/deploy/deploy-rustfs.sh"
        exit 0
        ;;
    -h|--help)
        echo "Usage: $0 [options]"
        echo ""
        echo "Options:"
        echo "  --fix-limits  Apply inotify limits (fix 'too many open files'), then exit"
        echo "  -h, --help    Show this help"
        exit 0
        ;;
esac

# Main entry
main "$@"
