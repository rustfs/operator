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

# RustFS cluster quick verification script
# Fully dynamic configuration reading, no hardcoding

set -e

# Configuration parameters (can be overridden via environment variables)
TENANT_NAME="${TENANT_NAME:-}"
NAMESPACE="${NAMESPACE:-}"

# If no parameters provided, try to get from command line arguments
if [ -z "$TENANT_NAME" ] && [ $# -gt 0 ]; then
    TENANT_NAME="$1"
fi
if [ -z "$NAMESPACE" ] && [ $# -gt 1 ]; then
    NAMESPACE="$2"
fi

# If still not found, try to find the first Tenant from cluster
if [ -z "$TENANT_NAME" ]; then
    # If namespace is specified, search in that namespace
    if [ -n "$NAMESPACE" ]; then
        TENANT_NAME=$(kubectl get tenants -n "$NAMESPACE" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || echo "")
    else
        # Search for first Tenant from all namespaces
        TENANT_NAME=$(kubectl get tenants --all-namespaces -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || echo "")
        if [ -n "$TENANT_NAME" ]; then
            NAMESPACE=$(kubectl get tenants --all-namespaces -o jsonpath='{.items[0].metadata.namespace}' 2>/dev/null || echo "")
        fi
    fi
    
    if [ -z "$TENANT_NAME" ]; then
        echo "Error: Tenant resource not found"
        echo "Usage: $0 [TENANT_NAME] [NAMESPACE]"
        echo "  Or set environment variables: TENANT_NAME=<tenant-name> NAMESPACE=<namespace> $0"
        exit 1
    fi
fi

# If namespace is not specified, read from Tenant resource
if [ -z "$NAMESPACE" ]; then
    # Try to find Tenant from all namespaces
    NAMESPACE=$(kubectl get tenant "$TENANT_NAME" --all-namespaces -o jsonpath='{.items[0].metadata.namespace}' 2>/dev/null || echo "")
    
    if [ -z "$NAMESPACE" ]; then
        echo "Error: Tenant '$TENANT_NAME' not found"
        exit 1
    fi
fi

# Verify Tenant exists
if ! kubectl get tenant "$TENANT_NAME" -n "$NAMESPACE" &>/dev/null; then
    echo "Error: Tenant '$TENANT_NAME' does not exist in namespace '$NAMESPACE'"
    exit 1
fi

echo "========================================="
echo "  RustFS Cluster Status Check"
echo "========================================="
echo "Tenant: $TENANT_NAME"
echo "Namespace: $NAMESPACE"
echo ""

# Check Tenant status
echo "1. Tenant status:"
kubectl get tenant "$TENANT_NAME" -n "$NAMESPACE"
echo ""

# Check Pod status
echo "2. Pod status:"
kubectl get pods -n "$NAMESPACE" -l "rustfs.tenant=$TENANT_NAME" -o wide
echo ""

# Check Services
echo "3. Services:"
kubectl get svc -n "$NAMESPACE" -l "rustfs.tenant=$TENANT_NAME"
echo ""

# Check PVCs
echo "4. Persistent Volume Claims (PVC):"
kubectl get pvc -n "$NAMESPACE" -l "rustfs.tenant=$TENANT_NAME"
echo ""

# Check StatefulSets
echo "5. StatefulSet:"
kubectl get statefulset -n "$NAMESPACE" -l "rustfs.tenant=$TENANT_NAME"
echo ""

# Check RUSTFS_VOLUMES configuration
echo "6. RustFS volume configuration:"
# Get first Pod name
FIRST_POD=$(kubectl get pods -n "$NAMESPACE" -l "rustfs.tenant=$TENANT_NAME" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || echo "")
if [ -n "$FIRST_POD" ]; then
    kubectl describe pod "$FIRST_POD" -n "$NAMESPACE" | grep "RUSTFS_VOLUMES:" -A 1 || echo "RUSTFS_VOLUMES configuration not found"
else
    echo "No Pod found"
fi
echo ""

# Show port forward commands
echo "========================================="
echo "  Access RustFS"
echo "========================================="
echo ""

# Dynamically get Service information
# Find all related Services by labels
SERVICES=$(kubectl get svc -n "$NAMESPACE" -l "rustfs.tenant=$TENANT_NAME" -o jsonpath='{.items[*].metadata.name}' 2>/dev/null || echo "")

# Find IO Service (port 9000) and Console Service (port 9001)
IO_SERVICE=""
CONSOLE_SERVICE=""

for SVC_NAME in $SERVICES; do
    # Check Service port
    SVC_PORT=$(kubectl get svc "$SVC_NAME" -n "$NAMESPACE" -o jsonpath='{.spec.ports[0].port}' 2>/dev/null || echo "")
    
    # IO Service typically uses port 9000
    if [ "$SVC_PORT" = "9000" ]; then
        IO_SERVICE="$SVC_NAME"
    fi
    
    # Console Service typically uses port 9001
    if [ "$SVC_PORT" = "9001" ]; then
        CONSOLE_SERVICE="$SVC_NAME"
    fi
done

# If not found by port, try to find by naming convention
if [ -z "$IO_SERVICE" ]; then
    # IO Service might be "rustfs" or contain "io"
    IO_SERVICE=$(kubectl get svc -n "$NAMESPACE" -l "rustfs.tenant=$TENANT_NAME" -o jsonpath='{.items[?(@.metadata.name=="rustfs")].metadata.name}' 2>/dev/null || echo "")
fi

if [ -z "$CONSOLE_SERVICE" ]; then
    # Console Service is typically "{tenant-name}-console"
    CONSOLE_SERVICE="${TENANT_NAME}-console"
    # Verify it exists
    if ! kubectl get svc "$CONSOLE_SERVICE" -n "$NAMESPACE" &>/dev/null; then
        CONSOLE_SERVICE=""
    fi
fi

# Show IO Service port forward information
if [ -n "$IO_SERVICE" ] && kubectl get svc "$IO_SERVICE" -n "$NAMESPACE" &>/dev/null; then
    IO_PORT=$(kubectl get svc "$IO_SERVICE" -n "$NAMESPACE" -o jsonpath='{.spec.ports[0].port}' 2>/dev/null || echo "")
    IO_TARGET_PORT=$(kubectl get svc "$IO_SERVICE" -n "$NAMESPACE" -o jsonpath='{.spec.ports[0].targetPort}' 2>/dev/null || echo "$IO_PORT")
    
    echo "S3 API port forward:"
    echo "  kubectl port-forward -n $NAMESPACE svc/$IO_SERVICE ${IO_PORT}:${IO_TARGET_PORT}"
    echo "  Access: http://localhost:${IO_PORT}"
    echo ""
else
    echo "⚠️  IO Service (S3 API) not found"
    echo ""
fi

# Show Console Service port forward information
if [ -n "$CONSOLE_SERVICE" ] && kubectl get svc "$CONSOLE_SERVICE" -n "$NAMESPACE" &>/dev/null; then
    CONSOLE_PORT=$(kubectl get svc "$CONSOLE_SERVICE" -n "$NAMESPACE" -o jsonpath='{.spec.ports[0].port}' 2>/dev/null || echo "")
    CONSOLE_TARGET_PORT=$(kubectl get svc "$CONSOLE_SERVICE" -n "$NAMESPACE" -o jsonpath='{.spec.ports[0].targetPort}' 2>/dev/null || echo "$CONSOLE_PORT")
    
    echo "Web Console port forward:"
    echo "  kubectl port-forward -n $NAMESPACE svc/$CONSOLE_SERVICE ${CONSOLE_PORT}:${CONSOLE_TARGET_PORT}"
    echo "  Access: http://localhost:${CONSOLE_PORT}/rustfs/console/index.html"
    echo ""
else
    echo "⚠️  Console Service (Web UI) not found"
    echo ""
fi

# Dynamically get credentials
echo "Credentials:"
CREDS_SECRET=$(kubectl get tenant "$TENANT_NAME" -n "$NAMESPACE" -o jsonpath='{.spec.credsSecret.name}' 2>/dev/null || echo "")

if [ -n "$CREDS_SECRET" ]; then
    # Read credentials from Secret
    ACCESS_KEY=$(kubectl get secret "$CREDS_SECRET" -n "$NAMESPACE" -o jsonpath='{.data.accesskey}' 2>/dev/null | base64 -d 2>/dev/null || echo "")
    SECRET_KEY=$(kubectl get secret "$CREDS_SECRET" -n "$NAMESPACE" -o jsonpath='{.data.secretkey}' 2>/dev/null | base64 -d 2>/dev/null || echo "")
    
    if [ -n "$ACCESS_KEY" ] && [ -n "$SECRET_KEY" ]; then
        echo "  Source: Secret '$CREDS_SECRET'"
        echo "  Access Key: $ACCESS_KEY"
        echo "  Secret Key: [hidden]"
    else
        echo "  ⚠️  Unable to read credentials from Secret '$CREDS_SECRET'"
    fi
else
    # Try to read from environment variables
    ROOT_USER=$(kubectl get tenant "$TENANT_NAME" -n "$NAMESPACE" -o jsonpath='{.spec.env[?(@.name=="RUSTFS_ROOT_USER")].value}' 2>/dev/null || echo "")
    ROOT_PASSWORD=$(kubectl get tenant "$TENANT_NAME" -n "$NAMESPACE" -o jsonpath='{.spec.env[?(@.name=="RUSTFS_ROOT_PASSWORD")].value}' 2>/dev/null || echo "")
    
    if [ -n "$ROOT_USER" ] && [ -n "$ROOT_PASSWORD" ]; then
        echo "  Source: Environment variables"
        echo "  Username: $ROOT_USER"
        echo "  Password: $ROOT_PASSWORD"
    else
        echo "  ⚠️  Credentials not configured"
        echo "  Note: RustFS may use built-in default credentials, please refer to RustFS documentation"
    fi
fi
echo ""

# Show cluster configuration
echo "========================================="
echo "  Cluster Configuration"
echo "========================================="
echo ""

# Read configuration from Tenant resource
POOLS=$(kubectl get tenant "$TENANT_NAME" -n "$NAMESPACE" -o jsonpath='{.spec.pools[*].name}' 2>/dev/null || echo "")
POOL_COUNT=$(echo "$POOLS" | wc -w | tr -d ' ')

if [ "$POOL_COUNT" -eq 0 ]; then
    echo "⚠️  No Pool configuration found"
else
    echo "Pool count: $POOL_COUNT"
    echo ""
    
    TOTAL_SERVERS=0
    TOTAL_VOLUMES=0
    
    # Iterate through each Pool
    for POOL_NAME in $POOLS; do
        SERVERS=$(kubectl get tenant "$TENANT_NAME" -n "$NAMESPACE" -o jsonpath="{.spec.pools[?(@.name==\"$POOL_NAME\")].servers}" 2>/dev/null || echo "0")
        VOLUMES_PER_SERVER=$(kubectl get tenant "$TENANT_NAME" -n "$NAMESPACE" -o jsonpath="{.spec.pools[?(@.name==\"$POOL_NAME\")].persistence.volumesPerServer}" 2>/dev/null || echo "0")
        STORAGE_SIZE=$(kubectl get tenant "$TENANT_NAME" -n "$NAMESPACE" -o jsonpath="{.spec.pools[?(@.name==\"$POOL_NAME\")].persistence.volumeClaimTemplate.resources.requests.storage}" 2>/dev/null || echo "")
        
        if [ -n "$SERVERS" ] && [ "$SERVERS" != "0" ] && [ -n "$VOLUMES_PER_SERVER" ] && [ "$VOLUMES_PER_SERVER" != "0" ]; then
            POOL_VOLUMES=$((SERVERS * VOLUMES_PER_SERVER))
            TOTAL_SERVERS=$((TOTAL_SERVERS + SERVERS))
            TOTAL_VOLUMES=$((TOTAL_VOLUMES + POOL_VOLUMES))
            
            echo "Pool: $POOL_NAME"
            echo "  Servers: $SERVERS"
            echo "  Volumes per server: $VOLUMES_PER_SERVER"
            echo "  Total volumes: $POOL_VOLUMES"
            
            if [ -n "$STORAGE_SIZE" ]; then
                # Extract number and unit
                STORAGE_NUM=$(echo "$STORAGE_SIZE" | sed 's/[^0-9]//g')
                STORAGE_UNIT=$(echo "$STORAGE_SIZE" | sed 's/[0-9]//g')
                if [ -n "$STORAGE_NUM" ] && [ "$STORAGE_NUM" != "0" ]; then
                    POOL_CAPACITY_NUM=$((POOL_VOLUMES * STORAGE_NUM))
                    echo "  Total capacity: ${POOL_CAPACITY_NUM}${STORAGE_UNIT} ($POOL_VOLUMES × $STORAGE_SIZE)"
                fi
            fi
            echo ""
        fi
    done
    
    # Show summary information
    if [ "$POOL_COUNT" -gt 1 ]; then
        echo "Summary:"
        echo "  Total servers: $TOTAL_SERVERS"
        echo "  Total volumes: $TOTAL_VOLUMES"
        
        # Try to calculate total capacity (if all Pools use same storage size)
        if [ -n "$STORAGE_SIZE" ]; then
            STORAGE_NUM=$(echo "$STORAGE_SIZE" | sed 's/[^0-9]//g')
            STORAGE_UNIT=$(echo "$STORAGE_SIZE" | sed 's/[0-9]//g')
            if [ -n "$STORAGE_NUM" ] && [ "$STORAGE_NUM" != "0" ]; then
                TOTAL_CAPACITY_NUM=$((TOTAL_VOLUMES * STORAGE_NUM))
                echo "  Total capacity: ${TOTAL_CAPACITY_NUM}${STORAGE_UNIT} ($TOTAL_VOLUMES × $STORAGE_SIZE)"
            fi
        fi
    fi
fi
