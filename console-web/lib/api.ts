import { apiClient } from "@/lib/api-client"
import type {
  TenantListResponse,
  TenantDetailsResponse,
  TenantListItem,
  CreateTenantRequest,
  UpdateTenantRequest,
  PoolListResponse,
  PoolDetails,
  AddPoolRequest,
  AddPoolResponse,
  DeletePoolResponse,
  PodListResponse,
  PodDetails,
  DeletePodResponse,
  EventListResponse,
  NodeListResponse,
  NamespaceListResponse,
  ClusterResourcesResponse,
  TenantYamlPayload,
} from "@/types/api"

const ns = (namespace: string) => `/namespaces/${encodeURIComponent(namespace)}`
const tenant = (namespace: string, name: string) => `${ns(namespace)}/tenants/${encodeURIComponent(name)}`
const pools = (namespace: string, name: string) => `${tenant(namespace, name)}/pools`
const pool = (namespace: string, name: string, poolName: string) =>
  `${pools(namespace, name)}/${encodeURIComponent(poolName)}`
const pods = (namespace: string, name: string) => `${tenant(namespace, name)}/pods`
const pod = (namespace: string, name: string, podName: string) =>
  `${pods(namespace, name)}/${encodeURIComponent(podName)}`
const events = (namespace: string, tenantName: string) =>
  `${ns(namespace)}/tenants/${encodeURIComponent(tenantName)}/events`
const tenantYaml = (namespace: string, name: string) => `${tenant(namespace, name)}/yaml`

// ----- Tenants -----
export async function listTenants(): Promise<TenantListResponse> {
  return apiClient.get<TenantListResponse>("/tenants")
}

export async function listTenantsByNamespace(namespace: string): Promise<TenantListResponse> {
  return apiClient.get<TenantListResponse>(`${ns(namespace)}/tenants`)
}

export async function getTenant(namespace: string, name: string): Promise<TenantDetailsResponse> {
  return apiClient.get<TenantDetailsResponse>(`${tenant(namespace, name)}`)
}

export async function createTenant(body: CreateTenantRequest): Promise<TenantListItem> {
  return apiClient.post<TenantListItem>("/tenants", body)
}

export async function updateTenant(
  namespace: string,
  name: string,
  body: UpdateTenantRequest,
): Promise<{ success: boolean; message: string; tenant: TenantListItem }> {
  const payload: Record<string, unknown> = {}
  if (body.image !== undefined) payload.image = body.image
  if (body.mount_path !== undefined) payload.mountPath = body.mount_path
  if (body.creds_secret !== undefined) payload.credsSecret = body.creds_secret
  if (body.env !== undefined) payload.env = body.env
  if (body.pod_management_policy !== undefined) payload.podManagementPolicy = body.pod_management_policy
  if (body.image_pull_policy !== undefined) payload.imagePullPolicy = body.image_pull_policy
  if (body.logging !== undefined) payload.logging = body.logging
  return apiClient.put(`${tenant(namespace, name)}`, Object.keys(payload).length ? payload : undefined)
}

export async function deleteTenant(namespace: string, name: string): Promise<{ success: boolean; message: string }> {
  return apiClient.delete(`${tenant(namespace, name)}`)
}

export async function getTenantYaml(namespace: string, name: string): Promise<TenantYamlPayload> {
  return apiClient.get<TenantYamlPayload>(tenantYaml(namespace, name))
}

export async function updateTenantYaml(
  namespace: string,
  name: string,
  body: TenantYamlPayload,
): Promise<TenantYamlPayload> {
  return apiClient.put<TenantYamlPayload>(tenantYaml(namespace, name), body)
}

// ----- Pools -----
export async function listPools(namespace: string, tenantName: string): Promise<PoolListResponse> {
  return apiClient.get<PoolListResponse>(`${pools(namespace, tenantName)}`)
}

export async function addPool(namespace: string, tenantName: string, body: AddPoolRequest): Promise<AddPoolResponse> {
  return apiClient.post<AddPoolResponse>(`${pools(namespace, tenantName)}`, body)
}

export async function deletePool(namespace: string, tenantName: string, poolName: string): Promise<DeletePoolResponse> {
  return apiClient.delete<DeletePoolResponse>(`${pool(namespace, tenantName, poolName)}`)
}

// ----- Pods -----
export async function listPods(namespace: string, tenantName: string): Promise<PodListResponse> {
  return apiClient.get<PodListResponse>(`${pods(namespace, tenantName)}`)
}

export async function getPod(namespace: string, tenantName: string, podName: string): Promise<PodDetails> {
  return apiClient.get<PodDetails>(`${pod(namespace, tenantName, podName)}`)
}

export async function deletePod(namespace: string, tenantName: string, podName: string): Promise<DeletePodResponse> {
  return apiClient.delete<DeletePodResponse>(`${pod(namespace, tenantName, podName)}`)
}

export async function restartPod(
  namespace: string,
  tenantName: string,
  podName: string,
  force = false,
): Promise<{ success: boolean; message: string }> {
  return apiClient.post(`${pod(namespace, tenantName, podName)}/restart`, {
    force,
  })
}

export async function getPodLogs(
  namespace: string,
  tenantName: string,
  podName: string,
  params?: { container?: string; tail_lines?: number; timestamps?: boolean },
): Promise<string> {
  const search = new URLSearchParams()
  if (params?.container) search.set("container", params.container)
  if (params?.tail_lines != null) search.set("tail_lines", String(params.tail_lines))
  if (params?.timestamps) search.set("timestamps", "true")
  const q = search.toString()
  return apiClient.getText(`${pod(namespace, tenantName, podName)}/logs${q ? `?${q}` : ""}`)
}

// ----- Events -----
export async function listTenantEvents(namespace: string, tenantName: string): Promise<EventListResponse> {
  return apiClient.get<EventListResponse>(events(namespace, tenantName))
}

// ----- Cluster -----
export async function listNodes(): Promise<NodeListResponse> {
  return apiClient.get<NodeListResponse>("/cluster/nodes")
}

export async function getClusterResources(): Promise<ClusterResourcesResponse> {
  return apiClient.get<ClusterResourcesResponse>("/cluster/resources")
}

export async function listNamespaces(): Promise<NamespaceListResponse> {
  return apiClient.get<NamespaceListResponse>("/namespaces")
}

export async function createNamespace(name: string): Promise<unknown> {
  return apiClient.post("/namespaces", { name })
}
