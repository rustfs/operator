export type TopologyTenantState = "Ready" | "Updating" | "Degraded" | "NotReady" | "Unknown"

export interface TopologyClusterSummary {
  nodes: number
  namespaces: number
  tenants: number
  unhealthy_tenants: number
  total_cpu: string
  total_memory: string
  allocatable_cpu: string
  allocatable_memory: string
}

export interface TopologyCluster {
  id: string
  name: string
  version: string
  summary: TopologyClusterSummary
}

export interface TopologyTenantSummary {
  pool_count: number
  replicas: number
  capacity: string
  capacity_bytes: number
  endpoint: string | null
  console_endpoint: string | null
}

export interface TopologyPool {
  name: string
  state: TopologyTenantState
  servers: number
  volumes_per_server: number
  replicas: number
  capacity: string
}

export interface TopologyPod {
  name: string
  pool: string
  phase: string
  ready: string
  node: string | null
}

export interface TopologyTenant {
  name: string
  namespace: string
  state: TopologyTenantState
  created_at: string | null
  summary: TopologyTenantSummary
  pools?: TopologyPool[]
  pods?: TopologyPod[]
}

export interface TopologyNamespace {
  name: string
  tenant_count: number
  unhealthy_tenant_count: number
  tenants: TopologyTenant[]
}

export interface TopologyNode {
  name: string
  status: string
  roles: string[]
  cpu_capacity: string
  memory_capacity: string
  cpu_allocatable: string
  memory_allocatable: string
}

export interface TopologyOverviewResponse {
  cluster: TopologyCluster
  namespaces: TopologyNamespace[]
  nodes: TopologyNode[]
}
