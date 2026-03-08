"use client"

import { useState } from "react"
import { useRouter } from "next/navigation"
import Link from "next/link"
import { useTranslation } from "react-i18next"
import { toast } from "sonner"
import { RiArrowLeftLine } from "@remixicon/react"
import { Page } from "@/components/page"
import { PageHeader } from "@/components/page-header"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Spinner } from "@/components/ui/spinner"
import { routes } from "@/lib/routes"
import * as api from "@/lib/api"
import type { CreatePoolRequest } from "@/types/api"
import { ApiError } from "@/lib/api-client"

const defaultPool: CreatePoolRequest = {
  name: "pool-0",
  servers: 2,
  volumes_per_server: 2,
  storage_size: "10Gi",
  storage_class: "",
}

export default function TenantCreatePage() {
  const { t } = useTranslation()
  const router = useRouter()
  const [name, setName] = useState("")
  const [namespace, setNamespace] = useState("default")
  const [pools, setPools] = useState<CreatePoolRequest[]>([{ ...defaultPool }])
  const [image, setImage] = useState("")
  const [credsSecret, setCredsSecret] = useState("")
  const [loading, setLoading] = useState(false)

  const updatePool = (index: number, field: keyof CreatePoolRequest, value: string | number) => {
    setPools((prev) => prev.map((p, i) => (i === index ? { ...p, [field]: value } : p)))
  }

  const addPool = () => {
    setPools((prev) => [
      ...prev,
      {
        name: `pool-${prev.length}`,
        servers: 2,
        volumes_per_server: 2,
        storage_size: "10Gi",
        storage_class: "",
      },
    ])
  }

  const removePool = (index: number) => {
    if (pools.length <= 1) return
    setPools((prev) => prev.filter((_, i) => i !== index))
  }

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!name.trim()) {
      toast.warning(t("Tenant name is required"))
      return
    }
    if (!namespace.trim()) {
      toast.warning(t("Namespace is required"))
      return
    }
    const validPools = pools.map((p) => ({
      ...p,
      storage_class: p.storage_class || undefined,
    }))
    setLoading(true)
    try {
      await api.createTenant({
        name: name.trim(),
        namespace: namespace.trim(),
        pools: validPools,
        image: image.trim() || undefined,
        creds_secret: credsSecret.trim() || undefined,
      })
      toast.success(t("Tenant created"))
      router.push(routes.tenantDetail(namespace.trim(), name.trim()))
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Create failed"))
    } finally {
      setLoading(false)
    }
  }

  return (
    <Page>
      <PageHeader
        sticky={false}
        actions={
          <Button asChild variant="outline" size="sm">
            <Link href={routes.tenants} prefetch={false}>
              <RiArrowLeftLine className="mr-1 size-4" />
              {t("Back")}
            </Link>
          </Button>
        }
      >
        <h1 className="text-lg font-semibold">{t("Create Tenant")}</h1>
      </PageHeader>

      <form onSubmit={handleSubmit} className="space-y-6">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">{t("Basic")}</CardTitle>
            <CardDescription>{t("Tenant name and namespace.")}</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="grid gap-4 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="name">{t("Name")}</Label>
                <Input id="name" value={name} onChange={(e) => setName(e.target.value)} placeholder="my-tenant" />
              </div>
              <div className="space-y-2">
                <Label htmlFor="namespace">{t("Namespace")}</Label>
                <Input
                  id="namespace"
                  value={namespace}
                  onChange={(e) => setNamespace(e.target.value)}
                  placeholder="default"
                />
              </div>
            </div>
            <div className="space-y-2">
              <Label htmlFor="image">
                {t("Image")} ({t("Optional")})
              </Label>
              <Input
                id="image"
                value={image}
                onChange={(e) => setImage(e.target.value)}
                placeholder="rustfs/rustfs:latest"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="creds">
                {t("Credentials Secret")} ({t("Optional")})
              </Label>
              <Input
                id="creds"
                value={credsSecret}
                onChange={(e) => setCredsSecret(e.target.value)}
                placeholder="secret-name"
              />
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-center justify-between">
            <div>
              <CardTitle className="text-base">{t("Pools")}</CardTitle>
              <CardDescription>{t("At least one pool with 4+ volumes (e.g. 2 servers × 2 volumes).")}</CardDescription>
            </div>
            <Button type="button" variant="outline" size="sm" onClick={addPool}>
              {t("Add Pool")}
            </Button>
          </CardHeader>
          <CardContent className="space-y-4">
            {pools.map((pool, index) => (
              <div key={index} className="rounded-lg border border-border p-4 space-y-4">
                <div className="flex justify-between items-center">
                  <span className="text-sm font-medium">
                    {t("Pool")} {index + 1}
                  </span>
                  {pools.length > 1 && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="xs"
                      className="text-destructive"
                      onClick={() => removePool(index)}
                    >
                      {t("Remove")}
                    </Button>
                  )}
                </div>
                <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-5">
                  <div className="space-y-2">
                    <Label>{t("Pool Name")}</Label>
                    <Input
                      value={pool.name}
                      onChange={(e) => updatePool(index, "name", e.target.value)}
                      placeholder="pool-0"
                    />
                  </div>
                  <div className="space-y-2">
                    <Label>{t("Servers")}</Label>
                    <Input
                      type="number"
                      min={1}
                      value={pool.servers}
                      onChange={(e) => updatePool(index, "servers", parseInt(e.target.value, 10) || 0)}
                    />
                  </div>
                  <div className="space-y-2">
                    <Label>{t("Volumes per Server")}</Label>
                    <Input
                      type="number"
                      min={1}
                      value={pool.volumes_per_server}
                      onChange={(e) => updatePool(index, "volumes_per_server", parseInt(e.target.value, 10) || 0)}
                    />
                  </div>
                  <div className="space-y-2">
                    <Label>{t("Storage Size")}</Label>
                    <Input
                      value={pool.storage_size}
                      onChange={(e) => updatePool(index, "storage_size", e.target.value)}
                      placeholder="10Gi"
                    />
                  </div>
                  <div className="space-y-2">
                    <Label>
                      {t("Storage Class")} ({t("Optional")})
                    </Label>
                    <Input
                      value={pool.storage_class || ""}
                      onChange={(e) => updatePool(index, "storage_class", e.target.value)}
                      placeholder=""
                    />
                  </div>
                </div>
              </div>
            ))}
          </CardContent>
        </Card>

        <div className="flex gap-2">
          <Button type="submit" disabled={loading}>
            {loading && <Spinner className="mr-2 size-4" />}
            {loading ? t("Creating...") : t("Create Tenant")}
          </Button>
          <Button asChild type="button" variant="outline">
            <Link href={routes.tenants} prefetch={false}>
              {t("Cancel")}
            </Link>
          </Button>
        </div>
      </form>
    </Page>
  )
}
