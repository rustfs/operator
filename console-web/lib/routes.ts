export const routes = {
  login: "/auth/login",
  dashboard: "/",
  tenants: "/tenants",
  tenantNew: "/tenants/new",
  tenantDetail: (namespace: string, name: string) =>
    `/tenants/${encodeURIComponent(namespace)}/${encodeURIComponent(name)}`,
  cluster: "/cluster",
}
