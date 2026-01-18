# RustFS Operator Development Guide

This guide will help you set up a local development environment for the RustFS Kubernetes Operator.

---

## 📋 Prerequisites

### Required Tools

1. **Rust Toolchain** (1.91+)
   - Project uses Rust Edition 2024
   - Required components: `rustfmt`, `clippy`, `rust-src`, `rust-analyzer`

2. **Kubernetes Cluster**
   - Kubernetes v1.27+ (current target: v1.30)
   - For local development, use:
     - [kind](https://kind.sigs.k8s.io/) (recommended)
     - [minikube](https://minikube.sigs.k8s.io/)
     - [k3s](https://k3s.io/)
     - Docker Desktop (built-in Kubernetes)

3. **kubectl**
   - For interacting with Kubernetes clusters

4. **Optional Tools**
   - `just` - Task runner (project includes Justfile)
   - `cargo-nextest` - Faster test runner
   - `docker` - For building container images
   - `OpenLens` - Kubernetes cluster management GUI

---

## 🚀 Quick Start

### 1. Install Rust Toolchain

The project uses `rust-toolchain.toml` to automatically manage the Rust version:

```bash
# If Rust is not installed yet
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Navigate to project directory (Rust will auto-install correct toolchain version)
cd ~/operator

# Verify installation
rustc --version
cargo --version
```

The toolchain will automatically install:
- `rustfmt` - Code formatter
- `clippy` - Code linter
- `rust-src` - Rust source code
- `rust-analyzer` - IDE support

### 2. Install Optional Development Tools

```bash
# Install cargo-nextest (faster test runner)
cargo install cargo-nextest

# Install just (task runner)
# macOS
brew install just

# Linux
# Download from https://github.com/casey/just/releases
# Or use package manager
```

### 3. Clone the Project (if not already done)

```bash
git clone https://github.com/rustfs/operator.git
cd operator
```

### 4. Verify Project Setup

```bash
# Check Rust toolchain
rustc --version  # Should be 1.91+

# Check project dependencies
cargo check

# Run formatting check
cargo fmt --all --check

# Run clippy check
cargo clippy --all-targets --all-features -- -D warnings
```

---

## 🔨 Building the Operator

### How to Compile the Operator

The operator can be built using Cargo (standard Rust build tool) or the Justfile task runner.

#### Method 1: Using Cargo (Standard)

```bash
# Debug build (faster compilation, larger binary, slower runtime)
cargo build

# Release build (slower compilation, smaller binary, faster runtime)
cargo build --release

# Binary locations:
# Debug:   target/debug/operator
# Release: target/release/operator
```

#### Method 2: Using Justfile (Recommended)

```bash
# Build Debug binary
just build

# Build Release binary
just build MODE=release
```

#### Build Output

After building, the operator binary will be located at:
- **Debug**: `target/debug/operator`
- **Release**: `target/release/operator`

You can run it directly:
```bash
# Run debug binary
./target/debug/operator --help

# Run release binary
./target/release/operator --help
```

#### Build Options

```bash
# Format code before building
just fmt && just build

# Run all checks before building
just pre-commit && just build MODE=release

# Clean and rebuild
cargo clean && cargo build --release
```

---

## 🐳 Installing kind

kind (Kubernetes in Docker) is the recommended tool for local Kubernetes development.

### Installation

#### macOS

```bash
# Using Homebrew (recommended)
brew install kind

# Verify installation
kind --version
```

#### Linux

```bash
# Download binary from releases
curl -Lo ./kind https://kind.sigs.k8s.io/dl/v0.20.0/kind-linux-amd64
chmod +x ./kind
sudo mv ./kind /usr/local/bin/kind

# Or using package manager (if available)
# Verify installation
kind --version
```

#### Windows

```bash
# Using Chocolatey
choco install kind

# Or download from: https://kind.sigs.k8s.io/docs/user/quick-start/
```

### Creating a kind Cluster

```bash
# Create a cluster named 'rustfs-dev'
kind create cluster --name rustfs-dev

# Verify cluster is running
kubectl cluster-info --context kind-rustfs-dev

# List clusters
kind get clusters

# Check cluster nodes
kubectl get nodes
```

### kind Cluster Management

#### Starting a Cluster

```bash
# If cluster exists but is stopped, restart it
# Note: kind clusters run in Docker containers, so they persist until deleted
# To "restart", you may need to recreate if Docker was restarted

# Check if cluster containers are running
docker ps | grep rustfs-dev

# If containers are stopped, restart Docker or recreate cluster
kind create cluster --name rustfs-dev
```

#### Stopping a Cluster

```bash
# kind clusters run in Docker containers
# To stop, you can stop Docker or delete the cluster

# Stop Docker Desktop (macOS/Windows)
# Or stop Docker daemon (Linux)
sudo systemctl stop docker

# Note: Stopping Docker will stop all kind clusters
```

#### Restarting a Cluster

```bash
# If Docker was restarted, kind clusters may need to be recreated
# Check cluster status
kind get clusters

# If cluster exists but kubectl can't connect, recreate it
kind delete cluster --name rustfs-dev
kind create cluster --name rustfs-dev

# Restore kubectl context
kubectl cluster-info --context kind-rustfs-dev
```

#### Deleting a Cluster

```bash
# Delete a specific cluster
kind delete cluster --name rustfs-dev

# Delete all kind clusters
kind delete cluster --all

# Verify deletion
kind get clusters
```

#### Advanced kind Configuration

Create a custom kind configuration file `kind-config.yaml`:

```yaml
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
- role: control-plane
  kubeadmConfigPatches:
  - |
    kind: InitConfiguration
    nodeRegistration:
      kubeletExtraArgs:
        node-labels: "ingress-ready=true"
  extraPortMappings:
  - containerPort: 80
    hostPort: 80
    protocol: TCP
  - containerPort: 443
    hostPort: 443
    protocol: TCP
```

Create cluster with custom config:
```bash
kind create cluster --name rustfs-dev --config kind-config.yaml
```

---

## 🖥️ Installing OpenLens

OpenLens is a powerful Kubernetes IDE for managing clusters visually.

### Installation

#### macOS

```bash
# Using Homebrew
brew install --cask openlens

# Or download from: https://github.com/MuhammedKalkan/OpenLens/releases
```

#### Linux

```bash
# Download AppImage from releases
wget https://github.com/MuhammedKalkan/OpenLens/releases/latest/download/OpenLens-<version>.AppImage
chmod +x OpenLens-<version>.AppImage
./OpenLens-<version>.AppImage

# Or install via Snap
snap install openlens
```

#### Windows

```bash
# Using Chocolatey
choco install openlens

# Or download installer from: https://github.com/MuhammedKalkan/OpenLens/releases
```

### Connecting OpenLens to kind Cluster

1. **Get kubeconfig path**:
   ```bash
   # kind stores kubeconfig in ~/.kube/config
   # Or get specific context
   kubectl config view --minify --context kind-rustfs-dev
   ```

2. **Open OpenLens**:
   - Click "Add Cluster" or "+" button
   - Select "Add from kubeconfig"
   - Navigate to `~/.kube/config` (or paste kubeconfig content)
   - Select context: `kind-rustfs-dev`
   - Click "Add"

3. **Verify Connection**:
   - You should see your kind cluster in the cluster list
   - Click on it to view nodes, pods, services, etc.

### Using OpenLens for Development

- **View Resources**: Browse Tenants, Pods, StatefulSets, Services
- **View Logs**: Click on any Pod to see logs
- **Terminal Access**: Open terminal in Pods directly
- **Resource Editor**: Edit YAML files directly
- **Event Viewer**: Monitor Kubernetes events in real-time

---

## 🏃 Installing and Running the Operator

### Step 1: Install CRD (Custom Resource Definition)

The operator requires the Tenant CRD to be installed in your cluster:

```bash
# Generate CRD YAML
cargo run -- crd > tenant-crd.yaml

# Or output directly to file
cargo run -- crd -f tenant-crd.yaml

# Install CRD
kubectl apply -f tenant-crd.yaml

# Verify CRD is installed
kubectl get crd tenants.rustfs.com

# View CRD details
kubectl describe crd tenants.rustfs.com
```

### Step 2: Configure kubectl Access

Ensure `kubectl` can access your cluster:

```bash
# Check current context
kubectl config current-context

# List all contexts
kubectl config get-contexts

# Switch to correct context (if needed)
kubectl config use-context kind-rustfs-dev

# Verify cluster connection
kubectl cluster-info
kubectl get nodes
```

### Step 3: Run Operator Locally (Development Mode)

#### Option A: Run from Source (Recommended for Development)

```bash
# Set log level (optional)
export RUST_LOG=debug
export RUST_LOG=rustfs_operator=debug,kube=info

# Run operator in debug mode
cargo run -- server

# Or run in release mode (faster)
cargo run --release -- server
```

The operator will:
- Connect to your Kubernetes cluster
- Watch for Tenant CRD changes
- Reconcile resources (StatefulSets, Services, RBAC)

#### Option B: Run Pre-built Binary

```bash
# Build the binary first
cargo build --release

# Run the binary
./target/release/operator server
```

#### Option C: Deploy as Pod in Cluster

```bash
# Build Docker image
docker build -t rustfs/operator:dev .

# Load image into kind cluster
kind load docker-image rustfs/operator:dev --name rustfs-dev

# Deploy using Helm (see deploy/README.md)
helm install rustfs-operator deploy/rustfs-operator/ \
  --namespace rustfs-system \
  --create-namespace \
  --set image.tag=dev \
  --set image.pullPolicy=Never
```

### Step 4: Test the Operator

In another terminal:

```bash
# Create a test Tenant
kubectl apply -f examples/minimal-dev-tenant.yaml

# Watch Tenant status
kubectl get tenant dev-minimal -w

# View created resources
kubectl get pods -l rustfs.tenant=dev-minimal
kubectl get statefulset -l rustfs.tenant=dev-minimal
kubectl get svc -l rustfs.tenant=dev-minimal
kubectl get pvc -l rustfs.tenant=dev-minimal
```

---

## 🐛 Debugging the Operator

### Debugging Methods

#### 1. Local Development Debugging

**Run with verbose logging**:
```bash
# Set detailed log levels
export RUST_LOG=debug
export RUST_LOG=rustfs_operator=debug,kube=info,tracing=debug

# Run operator
cargo run -- server
```

**Use a debugger** (VS Code):
1. Install "CodeLLDB" extension
2. Create `.vscode/launch.json`:
```json
{
    "version": "0.2.0",
    "configurations": [
        {
            "type": "lldb",
            "request": "launch",
            "name": "Debug Operator",
            "cargo": {
                "args": ["build", "--bin", "operator"],
                "filter": {
                    "name": "operator",
                    "kind": "bin"
                }
            },
            "args": ["server"],
            "cwd": "${workspaceFolder}",
            "env": {
                "RUST_LOG": "debug"
            }
        }
    ]
}
```
3. Set breakpoints and press F5

#### 2. Cluster-based Debugging

**View operator logs** (if deployed in cluster):
```bash
# Get operator pod name
kubectl get pods -n rustfs-system

# View logs
kubectl logs -f -n rustfs-system -l app.kubernetes.io/name=rustfs-operator

# View logs with timestamps
kubectl logs -f -n rustfs-system -l app.kubernetes.io/name=rustfs-operator --timestamps

# View previous logs (if pod restarted)
kubectl logs -f -n rustfs-system -l app.kubernetes.io/name=rustfs-operator --previous
```

**Debug operator pod**:
```bash
# Exec into operator pod
kubectl exec -it -n rustfs-system <operator-pod-name> -- /bin/sh

# Check environment variables
kubectl exec -n rustfs-system <operator-pod-name> -- env
```

#### 3. Resource Debugging

**Check reconciliation status**:
```bash
# View Tenant status
kubectl get tenant <tenant-name> -o yaml

# View Tenant events
kubectl describe tenant <tenant-name>

# View all events
kubectl get events --sort-by='.lastTimestamp' --all-namespaces

# Watch events in real-time
kubectl get events --watch --all-namespaces
```

**Check created resources**:
```bash
# View StatefulSet details
kubectl get statefulset -l rustfs.tenant=<tenant-name> -o yaml

# View Pod status
kubectl get pods -l rustfs.tenant=<tenant-name> -o wide

# View Pod logs
kubectl logs -f <pod-name> -l rustfs.tenant=<tenant-name>
```

---

## 📋 Logging and Log Locations

### Log Levels

The operator uses the `tracing` crate for structured logging. Log levels:

- `ERROR` - Errors that need attention
- `WARN` - Warnings about potential issues
- `INFO` - General informational messages
- `DEBUG` - Detailed debugging information
- `TRACE` - Very detailed tracing (very verbose)

### Setting Log Levels

#### Environment Variables

```bash
# Set global log level
export RUST_LOG=debug

# Set per-module log levels
export RUST_LOG=rustfs_operator=debug,kube=info,tracing=warn

# Common configurations:
# Development
export RUST_LOG=rustfs_operator=debug,kube=info

# Production
export RUST_LOG=rustfs_operator=info,kube=warn

# Troubleshooting
export RUST_LOG=rustfs_operator=trace,kube=debug
```

#### Log Location

**When running locally**:
- Logs are output to **stdout/stderr**
- View in terminal where operator is running
- Can redirect to file: `cargo run -- server 2>&1 | tee operator.log`

**When deployed in cluster**:
- Logs are stored in **Pod logs**
- View with: `kubectl logs -f <operator-pod-name> -n rustfs-system`
- Logs persist until Pod is deleted
- Use log aggregation tools (e.g., Loki, Fluentd) for long-term storage

### Viewing Logs

#### Local Development

```bash
# Terminal 1: Run operator with logging
export RUST_LOG=debug
cargo run -- server

# Terminal 2: View logs in real-time (if redirected to file)
tail -f operator.log

# Or use system log viewer (macOS)
log stream --predicate 'process == "operator"'
```

#### Cluster Deployment

```bash
# View current logs
kubectl logs -f -n rustfs-system -l app.kubernetes.io/name=rustfs-operator

# View logs with timestamps
kubectl logs -f -n rustfs-system -l app.kubernetes.io/name=rustfs-operator --timestamps

# View last 100 lines
kubectl logs --tail=100 -n rustfs-system -l app.kubernetes.io/name=rustfs-operator

# View logs since specific time
kubectl logs --since=10m -n rustfs-system -l app.kubernetes.io/name=rustfs-operator

# View logs from previous container (if pod restarted)
kubectl logs --previous -n rustfs-system -l app.kubernetes.io/name=rustfs-operator

# Export logs to file
kubectl logs -n rustfs-system -l app.kubernetes.io/name=rustfs-operator > operator.log
```

#### Using OpenLens

1. Open OpenLens
2. Select your cluster
3. Navigate to **Workloads** → **Pods**
4. Find operator pod in `rustfs-system` namespace
5. Click on pod → **Logs** tab
6. View real-time logs with filtering options

### Common Log Patterns

**Successful reconciliation**:
```
INFO reconcile: reconciled successful, object: <tenant-name>
```

**Reconciliation errors**:
```
ERROR reconcile: reconcile failed: <error-message>
WARN error_policy: <error-details>
```

**Resource creation**:
```
DEBUG Creating StatefulSet <name>
INFO StatefulSet <name> created successfully
```

**Status updates**:
```
DEBUG Updating tenant status: <status-details>
```

---

## 🧪 Running Tests

```bash
# Run all tests
cargo test

# Use nextest (faster)
cargo nextest run

# Or use just
just test

# Run specific test
cargo test test_statefulset_no_update_needed

# Run ignored tests (includes TLS tests)
cargo test -- --ignored

# Run tests with output
cargo test -- --nocapture

# Run tests in single thread (for debugging)
cargo test -- --test-threads=1
```

---

## 🛠️ Development Workflow

### Daily Development Process

1. **Create feature branch**
   ```bash
   git checkout -b feature/your-feature-name
   ```

2. **Write code**

3. **Format code**
   ```bash
   cargo fmt --all
   # or
   just fmt
   ```

4. **Run checks**
   ```bash
   just pre-commit
   # This runs:
   # - fmt-check
   # - clippy
   # - check
   # - test
   ```

5. **Run tests**
   ```bash
   cargo test
   # or
   just test
   ```

6. **Test operator locally**
   ```bash
   # Terminal 1: Run operator
   cargo run -- server
   
   # Terminal 2: Create test resources
   kubectl apply -f examples/minimal-dev-tenant.yaml
   kubectl get tenant -w
   ```

7. **Commit code**
   ```bash
   git add .
   git commit -m "feat: your feature description"
   ```

### Code Quality Checks

The project enforces strict code quality standards:

```bash
# Run all checks
just pre-commit

# Run individual checks
just fmt-check      # Check formatting
just clippy         # Code linting
just check          # Compilation check
just test           # Tests
```

**Note**: The project has `deny`-level clippy rules:
- `unwrap_used = "deny"` - Prohibits `unwrap()`
- `expect_used = "deny"` - Prohibits `expect()`

---

## 🧹 Cleaning Up

### Clean Test Resources

```bash
# Delete test Tenant (automatically deletes all related resources)
kubectl delete tenant dev-minimal

# Delete all Tenants
kubectl delete tenant --all
```

### Clean Cluster

```bash
# Delete kind cluster
kind delete cluster --name rustfs-dev

# Delete all kind clusters
kind delete cluster --all

# minikube
minikube delete
```

### Clean Build Artifacts

```bash
# Clean target directory
cargo clean

# Clean and rebuild
cargo clean && cargo build
```

---

## 🐛 Troubleshooting

### 1. Rust Version Mismatch

**Problem**: `error: toolchain 'stable' is not installed`

**Solution**:
```bash
# Navigate to project directory, rustup will auto-install correct toolchain
cd /Users/hongwei/my/operator
rustup show
```

### 2. Cannot Connect to Kubernetes Cluster

**Problem**: `Failed to connect to Kubernetes API`

**Solution**:
```bash
# Check kubectl configuration
kubectl config current-context
kubectl cluster-info

# Ensure cluster is running
kubectl get nodes

# For kind: check if cluster containers are running
docker ps | grep rustfs-dev
```

### 3. CRD Not Found

**Problem**: `the server could not find the requested resource`

**Solution**:
```bash
# Reinstall CRD
cargo run -- crd | kubectl apply -f -

# Verify CRD is installed
kubectl get crd tenants.rustfs.com
```

### 4. Clippy Errors

**Problem**: Clippy reports `unwrap_used` or `expect_used` errors

**Solution**:
- Use `Result` and `?` operator
- Use `match` or `if let` to handle `Option`
- Use `snafu` for error handling

### 5. Test Failures

**Problem**: Tests cannot run or fail

**Solution**:
```bash
# Run single test with detailed output
cargo test -- --nocapture test_name

# Run all tests (including ignored)
cargo test -- --include-ignored
```

### 6. kind Cluster Issues

**Problem**: Cannot connect to kind cluster after Docker restart

**Solution**:
```bash
# Recreate cluster
kind delete cluster --name rustfs-dev
kind create cluster --name rustfs-dev

# Restore kubectl context
kubectl cluster-info --context kind-rustfs-dev
```

---

## 📚 Useful Command Reference

### Cargo Commands

```bash
# Build
cargo build                    # Debug build
cargo build --release          # Release build

# Check
cargo check                    # Quick compilation check
cargo clippy                   # Code linting

# Test
cargo test                     # Run tests
cargo test -- --ignored        # Run ignored tests
cargo nextest run              # Use nextest

# Format
cargo fmt                      # Format code
cargo fmt --all --check        # Check formatting

# Documentation
cargo doc --open              # Generate and open docs
```

### kubectl Commands

```bash
# CRD operations
kubectl get crd               # List all CRDs
kubectl get tenant            # List all Tenants
kubectl describe tenant <name> # View Tenant details

# Resource operations
kubectl get pods -l rustfs.tenant=<name>
kubectl get statefulset -l rustfs.tenant=<name>
kubectl get svc -l rustfs.tenant=<name>

# Logs
kubectl logs -f <pod-name>
kubectl logs -f -l rustfs.tenant=<name>

# Events
kubectl get events --sort-by='.lastTimestamp'
```

### kind Commands

```bash
# Cluster management
kind create cluster --name <name>    # Create cluster
kind delete cluster --name <name>   # Delete cluster
kind get clusters                   # List clusters
kind get nodes --name <name>        # List nodes

# Image management
kind load docker-image <image> --name <cluster>  # Load image
```

---

## 🎯 Next Steps

- View [CONTRIBUTING.md](../CONTRIBUTING.md) for contribution guidelines
- View [DEVELOPMENT-NOTES.md](./DEVELOPMENT-NOTES.md) for development notes
- View [architecture-decisions.md](./architecture-decisions.md) for architecture decisions
- View [../examples/](../examples/) for usage examples

---

**Happy coding!** 🚀
