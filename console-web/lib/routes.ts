export const routes = {
  login: "/auth/login",
  dashboard: "/",
  tenants: "/tenants",
  tenantNew: "/tenants/new",
  tenantDetail: (namespace: string, name: string) =>
    `/tenants/detail?namespace=${encodeURIComponent(namespace)}&name=${encodeURIComponent(name)}`,
  cluster: "/cluster",
}
