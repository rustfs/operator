import { TenantDetailClient } from "./tenant-detail-client"

export function generateStaticParams() {
  return [{ namespace: "_", name: "_" }]
}

export default async function TenantDetailPage({
  params,
}: {
  params: Promise<{ namespace: string; name: string }>
}) {
  const { namespace, name } = await params
  return <TenantDetailClient namespace={namespace} name={name} />
}
