import { redirect } from "next/navigation"
import { routes } from "@/lib/routes"

export function generateStaticParams() {
  return [{ namespace: "_", name: "_" }]
}

/**
 * Legacy path /tenants/[namespace]/[name]. Redirect to the static-friendly
 * detail URL so client always hits /tenants/detail?namespace=...&name=...
 * (with static export only _/_ is built; real tenant paths would 404).
 */
export default async function TenantDetailRedirect({
  params,
}: {
  params: Promise<{ namespace: string; name: string }>
}) {
  const { namespace, name } = await params
  redirect(routes.tenantDetail(namespace, name))
}
