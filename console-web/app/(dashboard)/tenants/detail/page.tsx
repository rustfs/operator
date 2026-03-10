"use client"

import { Suspense, useEffect } from "react"
import { useSearchParams, useRouter } from "next/navigation"
import { TenantDetailClient } from "../[namespace]/[name]/tenant-detail-client"
import { Spinner } from "@/components/ui/spinner"
import { routes } from "@/lib/routes"

/**
 * Tenant detail page using query params (?namespace=...&name=...).
 * Used so the route is statically exportable (output: "export"); dynamic
 * segment routes like /tenants/[namespace]/[name] only get the paths from
 * generateStaticParams and real tenant names would 404.
 * useSearchParams is wrapped in Suspense to avoid client bailout / blank page.
 */
function TenantDetailContent() {
  const searchParams = useSearchParams()
  const router = useRouter()
  const namespace = searchParams.get("namespace")
  const name = searchParams.get("name")
  const tab = searchParams.get("tab")

  useEffect(() => {
    if (!namespace?.trim() || !name?.trim()) {
      router.replace(routes.tenants)
    }
  }, [namespace, name, router])

  if (!namespace?.trim() || !name?.trim()) {
    return (
      <div className="flex items-center justify-center py-12">
        <Spinner className="size-8" />
      </div>
    )
  }

  return <TenantDetailClient namespace={namespace.trim()} name={name.trim()} initialTab={tab} />
}

export default function TenantDetailPage() {
  return (
    <Suspense
      fallback={
        <div className="flex items-center justify-center py-12">
          <Spinner className="size-8" />
        </div>
      }
    >
      <TenantDetailContent />
    </Suspense>
  )
}
