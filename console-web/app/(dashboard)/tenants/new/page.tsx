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
import type { CreatePoolRequest, CreateTenantRequest, TenantListItem } from "@/types/api"
import { ApiError } from "@/lib/api-client"

type CreateMode = "form" | "yaml"

const DEFAULT_RUSTFS_IMAGE = "rustfs/rustfs:1.0.0-beta.10"

const defaultPool: CreatePoolRequest = {
  name: "pool-0",
  servers: 1,
  volumes_per_server: 1,
  storage_size: "10Gi",
  storage_class: "",
}

const defaultTenantYaml = `apiVersion: rustfs.com/v1alpha1
kind: Tenant
metadata:
  name: my-tenant
  namespace: default
spec:
  image: ${DEFAULT_RUSTFS_IMAGE}
  credsSecret:
    name: rustfs-creds
  pools:
    - name: pool-0
      servers: 1
      persistence:
        volumesPerServer: 1
        volumeClaimTemplate:
          accessModes:
            - ReadWriteOnce
          resources:
            requests:
              storage: 10Gi
`

export default function TenantCreatePage() {
  const { t } = useTranslation()
  const router = useRouter()
  const [mode, setMode] = useState<CreateMode>("form")
  const [name, setName] = useState("")
  const [namespace, setNamespace] = useState("default")
  const [pools, setPools] = useState<CreatePoolRequest[]>([{ ...defaultPool }])
  const [image, setImage] = useState(DEFAULT_RUSTFS_IMAGE)
  const [credsSecret, setCredsSecret] = useState("")
  const [securityContext, setSecurityContext] = useState({
    runAsUser: "",
    runAsGroup: "",
    fsGroup: "",
    runAsNonRoot: true,
  })
  const [yamlContent, setYamlContent] = useState(defaultTenantYaml)
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
    setLoading(true)

    try {
      let createdTenant: TenantListItem

      if (mode === "yaml") {
        createdTenant = await api.createTenantYaml({ yaml: yamlContent })
      } else {
        if (!name.trim()) {
          toast.warning(t("Tenant name is required"))
          return
        }
        if (!namespace.trim()) {
          toast.warning(t("Namespace is required"))
          return
        }
        const trimmedImage = image.trim()
        if (!trimmedImage) {
          toast.warning(t("Image is required"))
          return
        }
        const requestBody: CreateTenantRequest = {
          name: name.trim(),
          namespace: namespace.trim(),
          pools: pools.map((p) => ({
            ...p,
            storage_class: p.storage_class || undefined,
          })),
          image: trimmedImage,
          creds_secret: credsSecret.trim() || undefined,
          security_context: {
            runAsUser: securityContext.runAsUser ? parseInt(securityContext.runAsUser, 10) : undefined,
            runAsGroup: securityContext.runAsGroup ? parseInt(securityContext.runAsGroup, 10) : undefined,
            fsGroup: securityContext.fsGroup ? parseInt(securityContext.fsGroup, 10) : undefined,
            runAsNonRoot: securityContext.runAsNonRoot,
          },
        }
        createdTenant = await api.createTenant(requestBody)
      }

      toast.success(t("Tenant created"))
      router.push(routes.tenantDetail(createdTenant.namespace, createdTenant.name))
    } catch (e) {
      const err = e as ApiError
      const fallback = e instanceof Error ? e.message : t("Create failed")
      toast.error(err.message || fallback)
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
            <CardTitle className="text-base">{t("Create Mode")}</CardTitle>
            <CardDescription>{t("Choose form-based or YAML-based tenant creation.")}</CardDescription>
          </CardHeader>
          <CardContent>
            <div className="flex gap-2">
              <Button type="button" variant={mode === "form" ? "default" : "outline"} onClick={() => setMode("form")}>
                {t("Form")}
              </Button>
              <Button type="button" variant={mode === "yaml" ? "default" : "outline"} onClick={() => setMode("yaml")}>
                {t("YAML")}
              </Button>
            </div>
          </CardContent>
        </Card>

        {mode === "form" ? (
          <>
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
                  <Label htmlFor="image">{t("Image")}</Label>
                  <Input
                    id="image"
                    required
                    value={image}
                    onChange={(e) => setImage(e.target.value)}
                    placeholder="rustfs/rustfs:1.0.0-beta.10"
                  />
                  <p className="text-xs text-muted-foreground">
                    {t(
                      "Use YAML mode for custom repositories, mutable tags, or digest-qualified images and set runtime-default-image-ack to the exact image reference.",
                    )}
                  </p>
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
              <CardHeader>
                <CardTitle className="text-base">{t("SecurityContext")}</CardTitle>
                <CardDescription>
                  {t(
                    "Override Pod SecurityContext UID/GID fields (default: 10001/10001/10001). Use YAML mode for seccomp, container, and Pool-level settings. Optional.",
                  )}
                </CardDescription>
              </CardHeader>
              <CardContent className="space-y-4">
                <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
                  <div className="space-y-2">
                    <Label>{t("Run As User")}</Label>
                    <Input
                      type="number"
                      placeholder="10001"
                      value={securityContext.runAsUser}
                      onChange={(e) => setSecurityContext((s) => ({ ...s, runAsUser: e.target.value }))}
                    />
                  </div>
                  <div className="space-y-2">
                    <Label>{t("Run As Group")}</Label>
                    <Input
                      type="number"
                      placeholder="10001"
                      value={securityContext.runAsGroup}
                      onChange={(e) => setSecurityContext((s) => ({ ...s, runAsGroup: e.target.value }))}
                    />
                  </div>
                  <div className="space-y-2">
                    <Label>{t("FsGroup")}</Label>
                    <Input
                      type="number"
                      placeholder="10001"
                      value={securityContext.fsGroup}
                      onChange={(e) => setSecurityContext((s) => ({ ...s, fsGroup: e.target.value }))}
                    />
                  </div>
                  <div className="flex items-end gap-3 pb-2">
                    <label htmlFor="create-sec-nonroot" className="text-sm whitespace-nowrap">
                      {t("Do not run as Root")}
                    </label>
                    <input
                      id="create-sec-nonroot"
                      type="checkbox"
                      checked={securityContext.runAsNonRoot}
                      onChange={(e) => setSecurityContext((s) => ({ ...s, runAsNonRoot: e.target.checked }))}
                      className="h-4 w-4 rounded border-border"
                    />
                  </div>
                </div>
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="flex flex-row items-center justify-between">
                <div>
                  <CardTitle className="text-base">{t("Pools")}</CardTitle>
                  <CardDescription>{t("RustFS validates storage layout when the tenant starts.")}</CardDescription>
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
          </>
        ) : (
          <Card>
            <CardHeader>
              <CardTitle className="text-base">{t("Tenant YAML")}</CardTitle>
              <CardDescription>{t("Paste tenant YAML and create directly.")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-2">
              <Label htmlFor="tenant-yaml">{t("YAML Content")}</Label>
              <textarea
                id="tenant-yaml"
                value={yamlContent}
                onChange={(e) => setYamlContent(e.target.value)}
                className="dark:bg-input/30 border-input focus-visible:border-ring focus-visible:ring-ring/50 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive dark:aria-invalid:border-destructive/50 min-h-[420px] w-full rounded-none border bg-transparent px-2.5 py-2 font-mono text-xs transition-colors placeholder:text-muted-foreground focus-visible:ring-1 md:text-xs outline-none"
                spellCheck={false}
              />
            </CardContent>
          </Card>
        )}

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
