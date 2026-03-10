// API types aligned with backend (src/console/models)

// ----- Tenant -----
export interface PoolInfo {
  name: string
  servers: number
  volumes_per_server: number
}

export interface TenantListItem {
  name: string
  namespace: string
  pools: PoolInfo[]
  state: string
  created_at: string | null
}

export interface TenantListResponse {
  tenants: TenantListItem[]
}

export type TenantLifecycleState = "Ready" | "Updating" | "Degraded" | "NotReady" | "Unknown"

export interface TenantStateCountItem {
  state: string
  count: number
}

export type TenantStateCountsResponse =
  | TenantStateCountItem[]
  | Record<string, number>
  | {
      state_counts?: TenantStateCountItem[]
      counts?: TenantStateCountItem[]
    }

export interface ServicePort {
  name: string
  port: number
  target_port: string
}

export interface ServiceInfo {
  name: string
  service_type: string
  ports: ServicePort[]
}

export interface TenantDetailsResponse {
  name: string
  namespace: string
  pools: PoolInfo[]
  state: string
  image: string | null
  mount_path: string | null
  created_at: string | null
  services: ServiceInfo[]
}

export interface CreatePoolRequest {
  name: string
  servers: number
  volumes_per_server: number
  storage_size: string
  storage_class?: string
}

export interface CreateSecurityContextRequest {
  runAsUser?: number
  runAsGroup?: number
  fsGroup?: number
  runAsNonRoot?: boolean
}

export interface CreateTenantRequest {
  name: string
  namespace: string
  pools: CreatePoolRequest[]
  image?: string
  mount_path?: string
  creds_secret?: string
  security_context?: CreateSecurityContextRequest
}

export interface UpdateTenantRequest {
  image?: string
  mount_path?: string
  env?: { name: string; value?: string }[]
  creds_secret?: string
  pod_management_policy?: string
  image_pull_policy?: string
  logging?: {
    logType: string
    volumeSize?: string
    storageClass?: string
  }
}

export interface DeleteTenantResponse {
  success: boolean
  message: string
}

export interface UpdateTenantResponse {
  success: boolean
  message: string
  tenant: TenantListItem
}

export interface TenantYamlPayload {
  yaml: string
}

// ----- Pool -----
export interface PoolDetails {
  name: string
  servers: number
  volumes_per_server: number
  total_volumes: number
  storage_class: string | null
  volume_size: string | null
  replicas: number
  ready_replicas: number
  updated_replicas: number
  current_revision: string | null
  update_revision: string | null
  state: string
  created_at: string | null
}

export interface PoolListResponse {
  pools: PoolDetails[]
}

export interface AddPoolRequest {
  name: string
  servers: number
  volumesPerServer: number
  storageSize: string
  storageClass?: string
  nodeSelector?: Record<string, string>
  resources?: {
    requests?: { cpu?: string; memory?: string }
    limits?: { cpu?: string; memory?: string }
  }
}

export interface AddPoolResponse {
  success: boolean
  message: string
  pool: PoolDetails
}

export interface DeletePoolResponse {
  success: boolean
  message: string
  warning?: string
}

// ----- Pod -----
export interface PodListItem {
  name: string
  pool: string
  status: string
  phase: string
  node: string | null
  ready: string
  restarts: number
  age: string
  created_at: string | null
}

export interface PodListResponse {
  pods: PodListItem[]
}

export interface PodCondition {
  type: string
  status: string
  reason?: string
  message?: string
  last_transition_time?: string
}

export interface PodStatus {
  phase: string
  conditions: PodCondition[]
  host_ip?: string
  pod_ip?: string
  start_time?: string
}

export interface ContainerStateRunning {
  status: "Running"
  started_at?: string
}

export interface ContainerStateWaiting {
  status: "Waiting"
  reason?: string
  message?: string
}

export interface ContainerStateTerminated {
  status: "Terminated"
  reason?: string
  exit_code: number
  finished_at?: string
}

export type ContainerState = ContainerStateRunning | ContainerStateWaiting | ContainerStateTerminated

export interface ContainerInfo {
  name: string
  image: string
  ready: boolean
  restart_count: number
  state: ContainerState
}

export interface VolumeInfo {
  name: string
  volume_type: string
  claim_name?: string
}

export interface PodDetails {
  name: string
  namespace: string
  pool: string
  status: PodStatus
  containers: ContainerInfo[]
  volumes: VolumeInfo[]
  node: string | null
  ip: string | null
  labels: Record<string, string>
  annotations: Record<string, string>
  created_at: string | null
}

export interface DeletePodResponse {
  success: boolean
  message: string
}

// ----- Event -----
export interface EventItem {
  event_type: string
  reason: string
  message: string
  involved_object: string
  first_timestamp: string | null
  last_timestamp: string | null
  count: number
}

export interface EventListResponse {
  events: EventItem[]
}

// ----- Encryption -----
export interface AppRoleInfo {
  engine: string | null
  retrySeconds: number | null
}

export interface VaultInfo {
  endpoint: string
  engine: string | null
  namespace: string | null
  prefix: string | null
  authType: string | null
  appRole: AppRoleInfo | null
  tlsSkipVerify: boolean | null
  customCertificates: boolean | null
}

export interface LocalKmsInfo {
  keyDirectory: string | null
  masterKeyId: string | null
}

export interface SecurityContextInfo {
  runAsUser: number | null
  runAsGroup: number | null
  fsGroup: number | null
  runAsNonRoot: boolean | null
}

export interface EncryptionInfoResponse {
  enabled: boolean
  backend: string
  vault: VaultInfo | null
  local: LocalKmsInfo | null
  kmsSecretName: string | null
  pingSeconds: number | null
  securityContext: SecurityContextInfo | null
}

export interface UpdateEncryptionRequest {
  enabled: boolean
  backend?: string
  vault?: {
    endpoint: string
    engine?: string
    namespace?: string
    prefix?: string
    authType?: string
    appRole?: {
      engine?: string
      retrySeconds?: number
    }
    tlsSkipVerify?: boolean
    customCertificates?: boolean
  }
  local?: {
    keyDirectory?: string
    masterKeyId?: string
  }
  kmsSecretName?: string
  pingSeconds?: number
}

export interface EncryptionUpdateResponse {
  success: boolean
  message: string
}

export interface UpdateSecurityContextRequest {
  runAsUser?: number
  runAsGroup?: number
  fsGroup?: number
  runAsNonRoot?: boolean
}

export interface SecurityContextUpdateResponse {
  success: boolean
  message: string
}

// ----- Cluster -----
export interface NodeInfo {
  name: string
  status: string
  roles: string[]
  cpu_capacity: string
  memory_capacity: string
  cpu_allocatable: string
  memory_allocatable: string
}

export interface NodeListResponse {
  nodes: NodeInfo[]
}

export interface NamespaceItem {
  name: string
  status: string
  created_at?: string
}

export interface NamespaceListResponse {
  namespaces: NamespaceItem[]
}

export interface CreateNamespaceRequest {
  name: string
}

export interface ClusterResourcesResponse {
  total_nodes: number
  total_cpu: string
  total_memory: string
  allocatable_cpu: string
  allocatable_memory: string
}
