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
# For quick deployment and CRD modification verification

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

# Check required tools
check_prerequisites() {
    log_info "Checking required tools..."

    local missing_tools=()

    command -v kubectl >/dev/null 2>&1 || missing_tools+=("kubectl")
    command -v cargo >/dev/null 2>&1 || missing_tools+=("cargo")
    command -v kind >/dev/null 2>&1 || missing_tools+=("kind")

    if [ ${#missing_tools[@]} -ne 0 ]; then
        log_error "Missing required tools: ${missing_tools[*]}"
        exit 1
    fi

    log_success "All required tools are installed"
}

# Check Kubernetes cluster connection
check_cluster() {
    log_info "Checking Kubernetes cluster connection..."

    if ! kubectl cluster-info >/dev/null 2>&1; then
        log_error "Unable to connect to Kubernetes cluster"
        log_info "Attempting to start kind cluster..."

        if kind get clusters | grep -q "rustfs-dev"; then
            log_info "Detected kind cluster 'rustfs-dev', attempting to restart..."
            kind delete cluster --name rustfs-dev
        fi

        log_info "Creating new kind cluster..."
        kind create cluster --name rustfs-dev
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

# Start operator (background)
start_operator() {
    log_info "Starting operator..."

    # Check if operator is already running
    if pgrep -f "target/release/operator.*server" >/dev/null; then
        log_warning "Detected existing operator process"
        log_info "Stopping old operator process..."
        pkill -f "target/release/operator.*server" || true
        sleep 2
    fi

    # Start new operator process (background)
    nohup cargo run --release -- server > operator.log 2>&1 &
    OPERATOR_PID=$!
    echo $OPERATOR_PID > operator.pid

    log_success "Operator started (PID: $OPERATOR_PID)"
    log_info "Log file: operator.log"

    # Wait for operator to start
    sleep 3
}

# Start console (background)
start_console() {
    log_info "Starting console..."

    # Check if console is already running
    if pgrep -f "target/release/operator.*console" >/dev/null; then
        log_warning "Detected existing console process"
        log_info "Stopping old console process..."
        pkill -f "target/release/operator.*console" || true
        sleep 2
    fi

    # Start new console process (background)
    nohup cargo run --release -- console --port 9090 > console.log 2>&1 &
    CONSOLE_PID=$!
    echo $CONSOLE_PID > console.pid

    log_success "Console started (PID: $CONSOLE_PID)"
    log_info "Log file: console.log"

    # Wait for console to start
    sleep 2
}

# Deploy Tenant (EC 2+1 configuration)
deploy_tenant() {
    log_info "Deploying RustFS Tenant (using examples/simple-tenant.yaml)..."

    kubectl apply -f examples/simple-tenant.yaml

    log_success "Tenant submitted"
}

# Wait for pods to be ready
wait_for_pods() {
    log_info "Waiting for pods to start (max 5 minutes)..."

    local timeout=300
    local elapsed=0
    local interval=5

    while [ $elapsed -lt $timeout ]; do
        local ready_count=$(kubectl get pods -n rustfs-system --no-headers 2>/dev/null | grep -c "Running" || echo "0")
        local total_count=$(kubectl get pods -n rustfs-system --no-headers 2>/dev/null | wc -l || echo "0")

        if [ "$ready_count" -eq 2 ] && [ "$total_count" -eq 2 ]; then
            log_success "All pods are ready (2/2 Running)"
            return 0
        fi

        echo -ne "${BLUE}[INFO]${NC} Pod status: $ready_count/2 Running, waited ${elapsed}s...\r"
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

    log_info "1. Tenant status:"
    kubectl get tenant -n rustfs-system
    echo ""

    log_info "2. Pod status:"
    kubectl get pods -n rustfs-system -o wide
    echo ""

    log_info "3. Service status:"
    kubectl get svc -n rustfs-system
    echo ""

    log_info "4. PVC status:"
    kubectl get pvc -n rustfs-system
    echo ""

    log_info "5. StatefulSet status:"
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
    echo "  kubectl logs -f example-tenant-primary-0 -n rustfs-system"
    echo ""

    echo "🔌 Port forward S3 API (9000):"
    echo "  kubectl port-forward -n rustfs-system svc/rustfs 9000:9000"
    echo ""

    echo "🌐 Port forward RustFS Web Console (9001):"
    echo "  kubectl port-forward -n rustfs-system svc/example-tenant-console 9001:9001"
    echo ""

    echo "🖥️  Operator Console (Management API):"
    echo "  Listening on: http://localhost:9090"
    echo "  Health check: curl http://localhost:9090/healthz"
    echo ""

    echo "🔐 RustFS Credentials:"
    echo "  Username: admin"
    echo "  Password: admin123"
    echo ""

    echo "🔑 Operator Console Login:"
    echo "  Create K8s token: kubectl create token default --duration=24h"
    echo "  Login: POST http://localhost:9090/api/v1/login"
    echo "  Docs: deploy/console/README.md"
    echo ""

    echo "📊 Check cluster status:"
    echo "  ./check-rustfs.sh"
    echo ""

    echo "🗑️  Cleanup deployment:"
    echo "  ./cleanup-rustfs.sh"
    echo ""

    echo "📝 Logs:"
    echo "  Operator: tail -f operator.log"
    echo "  Console:  tail -f console.log"
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
    start_operator
    start_console
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

# 执行主流程
main "$@"
