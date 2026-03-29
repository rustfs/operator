"use client"

import { useRouter } from "next/navigation"
import Link from "next/link"
import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { toast } from "sonner"
import {
  RiArrowLeftLine,
  RiDeleteBinLine,
  RiAddLine,
  RiFileCopyLine,
  RiFileList3Line,
  RiRestartLine,
} from "@remixicon/react"
import { Page } from "@/components/page"
import { PageHeader } from "@/components/page-header"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Spinner } from "@/components/ui/spinner"
import { routes } from "@/lib/routes"
import * as api from "@/lib/api"
import type {
  TenantDetailsResponse,
  PoolDetails,
  PodListItem,
  EventItem,
  EventListResponse,
  AddPoolRequest,
  EncryptionInfoResponse,
  UpdateEncryptionRequest,
} from "@/types/api"
import { ApiError } from "@/lib/api-client"

type Tab = "overview" | "edit" | "pools" | "pods" | "events" | "encryption" | "security"

interface TenantDetailClientProps {
  namespace: string
  name: string
  initialTab?: string | null
  initialYamlEditable?: boolean
}

function normalizeTab(value?: string | null): Tab {
  switch ((value ?? "").toLowerCase()) {
    case "edit":
    case "yaml":
      return "edit"
    case "pools":
      return "pools"
    case "pods":
      return "pods"
    case "events":
      return "events"
    case "encryption":
      return "encryption"
    case "security":
      return "security"
    default:
      return "overview"
  }
}

export function TenantDetailClient({ namespace, name, initialTab, initialYamlEditable }: TenantDetailClientProps) {
  const router = useRouter()
  const { t } = useTranslation()

  const [tab, setTab] = useState<Tab>(() => normalizeTab(initialTab))
  const [tenant, setTenant] = useState<TenantDetailsResponse | null>(null)
  const [pools, setPools] = useState<PoolDetails[]>([])
  const [pods, setPods] = useState<PodListItem[]>([])
  const [events, setEvents] = useState<EventItem[]>([])
  const [eventsLoading, setEventsLoading] = useState(false)
  const [loading, setLoading] = useState(true)
  const [deleting, setDeleting] = useState(false)
  const [addPoolOpen, setAddPoolOpen] = useState(false)
  const [addPoolForm, setAddPoolForm] = useState<AddPoolRequest>({
    name: "pool-new",
    servers: 2,
    volumesPerServer: 2,
    storageSize: "10Gi",
    storageClass: "",
  })
  const [addPoolLoading, setAddPoolLoading] = useState(false)
  const [restartingPod, setRestartingPod] = useState<string | null>(null)
  const [deletingPod, setDeletingPod] = useState<string | null>(null)
  const [deletingPool, setDeletingPool] = useState<string | null>(null)
  const [logsPod, setLogsPod] = useState<string | null>(null)
  const [logsContent, setLogsContent] = useState("")
  const [logsLoading, setLogsLoading] = useState(false)
  const [tenantYaml, setTenantYaml] = useState("")
  const [tenantYamlSnapshot, setTenantYamlSnapshot] = useState("")
  const [tenantYamlLoaded, setTenantYamlLoaded] = useState(false)
  const [tenantYamlLoading, setTenantYamlLoading] = useState(false)
  const [isYamlEditable, setIsYamlEditable] = useState(!!initialYamlEditable)
  const [editLoading, setEditLoading] = useState(false)

  // Encryption tab state
  const [encLoaded, setEncLoaded] = useState(false)
  const [encLoading, setEncLoading] = useState(false)
  const [encSaving, setEncSaving] = useState(false)
  const [encEnabled, setEncEnabled] = useState(false)
  const [encBackend, setEncBackend] = useState<"local" | "vault">("local")
  const [encVault, setEncVault] = useState({
    endpoint: "",
    engine: "",
    namespace: "",
    prefix: "",
    authType: "token",
  })
  const [encAppRole, setEncAppRole] = useState({
    engine: "",
    retrySeconds: "",
  })
  const [encLocal, setEncLocal] = useState({
    keyDirectory: "",
    masterKeyId: "",
  })
  const [encKmsSecretName, setEncKmsSecretName] = useState("")
  const [encPingSeconds, setEncPingSeconds] = useState("")

  // Security tab state
  const [secCtxLoaded, setSecCtxLoaded] = useState(false)
  const [secCtxLoading, setSecCtxLoading] = useState(false)
  const [secCtxSaving, setSecCtxSaving] = useState(false)
  const [secCtx, setSecCtx] = useState({
    runAsUser: "",
    runAsGroup: "",
    fsGroup: "",
    runAsNonRoot: true,
  })

  const loadTenant = async () => {
    const [detailResult, poolResult, podResult] = await Promise.allSettled([
      api.getTenant(namespace, name),
      api.listPools(namespace, name),
      api.listPods(namespace, name),
    ])

    const detailOk = detailResult.status === "fulfilled"
    const poolOk = poolResult.status === "fulfilled"
    const podOk = podResult.status === "fulfilled"

    if (detailOk && poolOk && podOk) {
      setTenant(detailResult.value)
      setPools(poolResult.value.pools)
      setPods(podResult.value.pods)
    } else {
      const err = !detailOk
        ? (detailResult as PromiseRejectedResult).reason
        : !poolOk
          ? (poolResult as PromiseRejectedResult).reason
          : (podResult as PromiseRejectedResult).reason
      toast.error((err as ApiError).message || t("Failed to load tenant"))
    }
    setLoading(false)
  }

  const loadTenantYaml = async () => {
    setTenantYamlLoading(true)
    try {
      const res = await api.getTenantYaml(namespace, name)
      setTenantYaml(res.yaml)
      setTenantYamlSnapshot(res.yaml)
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Failed to load tenant YAML"))
    } finally {
      setTenantYamlLoaded(true)
      setTenantYamlLoading(false)
    }
  }

  useEffect(() => {
    loadTenant()
  }, [namespace, name]) // eslint-disable-line react-hooks/exhaustive-deps -- reload when route params change

  useEffect(() => {
    setEvents([])
  }, [namespace, name])

  useEffect(() => {
    if (tab !== "events") return
    setEventsLoading(true)
    let cleaned = false
    const url = api.getTenantEventsStreamUrl(namespace, name)
    const es = new EventSource(url, { withCredentials: true })
    es.onmessage = (ev) => {
      try {
        const data = JSON.parse(ev.data) as EventListResponse
        setEvents(data.events ?? [])
      } catch {
        /* ignore malformed chunk */
      }
      setEventsLoading(false)
    }
    es.onerror = () => {
      if (cleaned) return
      toast.error(t("Events stream could not be loaded"))
      setEventsLoading(false)
    }
    return () => {
      cleaned = true
      es.close()
    }
  }, [tab, namespace, name, t])

  useEffect(() => {
    setTenantYaml("")
    setTenantYamlSnapshot("")
    setTenantYamlLoaded(false)
    setIsYamlEditable(!!initialYamlEditable)
  }, [namespace, name, initialYamlEditable])

  useEffect(() => {
    setTab(normalizeTab(initialTab))
    setIsYamlEditable(!!initialYamlEditable)
  }, [namespace, name, initialTab, initialYamlEditable])

  useEffect(() => {
    if (tab !== "edit" || tenantYamlLoaded || tenantYamlLoading) return
    loadTenantYaml()
  }, [tab, tenantYamlLoaded, tenantYamlLoading]) // eslint-disable-line react-hooks/exhaustive-deps -- only lazy-load once per tenant

  useEffect(() => {
    if (tab !== "encryption" || encLoaded || encLoading) return
    loadEncryption()
  }, [tab, encLoaded, encLoading]) // eslint-disable-line react-hooks/exhaustive-deps -- only lazy-load once per tenant

  useEffect(() => {
    if (tab !== "security" || secCtxLoaded || secCtxLoading) return
    loadSecurityContext()
  }, [tab, secCtxLoaded, secCtxLoading]) // eslint-disable-line react-hooks/exhaustive-deps -- only lazy-load once per tenant

  const loadSecurityContext = async () => {
    setSecCtxLoading(true)
    try {
      const data = await api.getSecurityContext(namespace, name)
      setSecCtx({
        runAsUser: data.runAsUser?.toString() ?? "",
        runAsGroup: data.runAsGroup?.toString() ?? "",
        fsGroup: data.fsGroup?.toString() ?? "",
        runAsNonRoot: data.runAsNonRoot ?? true,
      })
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Failed to load security context"))
    } finally {
      setSecCtxLoaded(true)
      setSecCtxLoading(false)
    }
  }

  const handleSaveSecurityContext = async (e: React.FormEvent) => {
    e.preventDefault()
    setSecCtxSaving(true)
    try {
      await api.updateSecurityContext(namespace, name, {
        runAsUser: secCtx.runAsUser ? parseInt(secCtx.runAsUser, 10) : undefined,
        runAsGroup: secCtx.runAsGroup ? parseInt(secCtx.runAsGroup, 10) : undefined,
        fsGroup: secCtx.fsGroup ? parseInt(secCtx.fsGroup, 10) : undefined,
        runAsNonRoot: secCtx.runAsNonRoot,
      })
      toast.success(t("SecurityContext updated"))
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Update failed"))
    } finally {
      setSecCtxSaving(false)
    }
  }

  const handleDeleteTenant = async () => {
    if (!confirm(t("Delete this tenant? This cannot be undone."))) return
    setDeleting(true)
    try {
      await api.deleteTenant(namespace, name)
      toast.success(t("Tenant deleted"))
      router.push(routes.tenants)
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Delete failed"))
    } finally {
      setDeleting(false)
    }
  }

  const handleAddPool = async (e: React.FormEvent) => {
    e.preventDefault()
    setAddPoolLoading(true)
    try {
      await api.addPool(namespace, name, {
        ...addPoolForm,
        storageClass: addPoolForm.storageClass || undefined,
      })
      toast.success(t("Pool added"))
      setAddPoolOpen(false)
      loadTenant()
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Add pool failed"))
    } finally {
      setAddPoolLoading(false)
    }
  }

  const handleDeletePool = async (poolName: string) => {
    if (!confirm(t('Delete pool "{{name}}"?', { name: poolName }))) return
    setDeletingPool(poolName)
    try {
      await api.deletePool(namespace, name, poolName)
      toast.success(t("Pool deleted"))
      loadTenant()
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Delete failed"))
    } finally {
      setDeletingPool(null)
    }
  }

  const handleRestartPod = async (podName: string) => {
    setRestartingPod(podName)
    try {
      await api.restartPod(namespace, name, podName)
      toast.success(t("Pod restart requested"))
      loadTenant()
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Restart failed"))
    } finally {
      setRestartingPod(null)
    }
  }

  const handleDeletePod = async (podName: string) => {
    if (!confirm(t('Delete pod "{{name}}"?', { name: podName }))) return
    setDeletingPod(podName)
    try {
      await api.deletePod(namespace, name, podName)
      toast.success(t("Pod deleted"))
      loadTenant()
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Delete failed"))
    } finally {
      setDeletingPod(null)
    }
  }

  const loadLogs = async (podName: string) => {
    setLogsPod(podName)
    setLogsLoading(true)
    setLogsContent("")
    try {
      const text = await api.getPodLogs(namespace, name, podName, {
        tail_lines: 100,
        timestamps: true,
      })
      setLogsContent(text)
    } catch (e) {
      const err = e as ApiError
      setLogsContent(err.message || t("Failed to load logs"))
    } finally {
      setLogsLoading(false)
    }
  }

  const handleUpdateTenantYaml = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!tenantYaml.trim()) {
      toast.warning(t("YAML content is required"))
      return
    }
    setEditLoading(true)
    try {
      const res = await api.updateTenantYaml(namespace, name, { yaml: tenantYaml })
      setTenantYaml(res.yaml)
      setTenantYamlSnapshot(res.yaml)
      setIsYamlEditable(false)
      toast.success(t("Tenant YAML updated"))
      loadTenant()
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Update failed"))
    } finally {
      setEditLoading(false)
    }
  }

  const loadEncryption = async () => {
    setEncLoading(true)
    try {
      const data = await api.getEncryption(namespace, name)
      setEncEnabled(data.enabled)
      setEncBackend((data.backend === "vault" ? "vault" : "local") as "local" | "vault")
      if (data.vault) {
        setEncVault({
          endpoint: data.vault.endpoint || "",
          engine: data.vault.engine || "",
          namespace: data.vault.namespace || "",
          prefix: data.vault.prefix || "",
          authType: data.vault.authType || "token",
        })
        if (data.vault.appRole) {
          setEncAppRole({
            engine: data.vault.appRole.engine || "",
            retrySeconds: data.vault.appRole.retrySeconds?.toString() || "",
          })
        }
      }
      if (data.local) {
        setEncLocal({
          keyDirectory: data.local.keyDirectory || "",
          masterKeyId: data.local.masterKeyId || "",
        })
      }
      setEncKmsSecretName(data.kmsSecretName || "")
      setEncPingSeconds(data.pingSeconds?.toString() || "")
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Failed to load encryption config"))
    } finally {
      setEncLoaded(true)
      setEncLoading(false)
    }
  }

  const handleSaveEncryption = async (e: React.FormEvent) => {
    e.preventDefault()
    if (encEnabled && encBackend === "vault" && !encVault.endpoint.trim()) {
      toast.warning(t("Vault endpoint is required"))
      return
    }
    setEncSaving(true)
    try {
      const body: UpdateEncryptionRequest = {
        enabled: encEnabled,
        backend: encBackend,
        kmsSecretName: encKmsSecretName || undefined,
        pingSeconds: encPingSeconds ? parseInt(encPingSeconds, 10) : undefined,
      }
      if (encBackend === "vault") {
        body.vault = {
          endpoint: encVault.endpoint,
          engine: encVault.engine || undefined,
          namespace: encVault.namespace || undefined,
          prefix: encVault.prefix || undefined,
          authType: encVault.authType || undefined,
        }
        if (encVault.authType === "approle") {
          body.vault.appRole = {
            engine: encAppRole.engine || undefined,
            retrySeconds: encAppRole.retrySeconds ? parseInt(encAppRole.retrySeconds, 10) : undefined,
          }
        }
      } else {
        body.local = {
          keyDirectory: encLocal.keyDirectory || undefined,
          masterKeyId: encLocal.masterKeyId || undefined,
        }
      }
      const res = await api.updateEncryption(namespace, name, body)
      toast.success(res.message || t("Encryption config updated"))
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Update failed"))
    } finally {
      setEncSaving(false)
    }
  }

  const handleCopyYaml = async () => {
    try {
      await navigator.clipboard.writeText(tenantYaml)
      toast.success(t("YAML copied"))
    } catch {
      toast.error(t("Copy failed"))
    }
  }

  if (loading || !tenant) {
    return (
      <div className="flex items-center justify-center py-12">
        <Spinner className="size-8" />
      </div>
    )
  }

  const tabs: { id: Tab; labelKey: string }[] = [
    { id: "overview", labelKey: "Overview" },
    { id: "pools", labelKey: "Pools" },
    { id: "pods", labelKey: "Pods" },
    { id: "events", labelKey: "Events" },
    { id: "encryption", labelKey: "Encryption" },
    { id: "security", labelKey: "Security" },
    { id: "edit", labelKey: "YAML" },
  ]

  return (
    <Page>
      <PageHeader
        actions={
          <div className="flex gap-2">
            <Button asChild variant="outline" size="sm">
              <Link href={routes.tenants} prefetch={false}>
                <RiArrowLeftLine className="mr-1 size-4" />
                {t("Back")}
              </Link>
            </Button>
            <Button variant="destructive" size="sm" disabled={deleting} onClick={handleDeleteTenant}>
              {deleting ? <Spinner className="mr-1 size-4" /> : <RiDeleteBinLine className="mr-1 size-4" />}
              {t("Delete Tenant")}
            </Button>
          </div>
        }
      >
        <h1 className="text-lg font-semibold">
          {tenant.name} <span className="text-muted-foreground">/ {tenant.namespace}</span>
        </h1>
        <p className="text-sm text-muted-foreground">
          {t("State")}: {tenant.state}
        </p>
      </PageHeader>

      <div className="flex gap-2 border-b border-border mb-4">
        {tabs.map(({ id, labelKey }) => (
          <button
            key={id}
            type="button"
            onClick={() => setTab(id)}
            className={`px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors ${
              tab === id
                ? "border-primary text-primary"
                : "border-transparent text-muted-foreground hover:text-foreground"
            }`}
          >
            {t(labelKey)}
          </button>
        ))}
      </div>

      {tab === "overview" && (
        <div className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle className="text-base">{t("Details")}</CardTitle>
            </CardHeader>
            <CardContent className="space-y-2 text-sm">
              <p>
                <span className="text-muted-foreground">{t("Image")}:</span> {tenant.image || "-"}
              </p>
              <p>
                <span className="text-muted-foreground">{t("Mount Path")}:</span> {tenant.mount_path || "-"}
              </p>
              <p>
                <span className="text-muted-foreground">{t("Created")}:</span>{" "}
                {tenant.created_at ? new Date(tenant.created_at).toLocaleString() : "-"}
              </p>
            </CardContent>
          </Card>
          {tenant.services.length > 0 && (
            <Card>
              <CardHeader>
                <CardTitle className="text-base">{t("Services")}</CardTitle>
                <CardDescription>
                  {t("Only services created for this tenant are listed (operator/console services are not included).")}
                </CardDescription>
              </CardHeader>
              <CardContent>
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>{t("Name")}</TableHead>
                      <TableHead>{t("Type")}</TableHead>
                      <TableHead>{t("Ports")}</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {tenant.services.map((svc) => (
                      <TableRow key={svc.name}>
                        <TableCell>{svc.name}</TableCell>
                        <TableCell>{svc.service_type}</TableCell>
                        <TableCell>{svc.ports.map((p) => `${p.name}:${p.port}`).join(", ")}</TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </CardContent>
            </Card>
          )}
        </div>
      )}

      {tab === "edit" && (
        <Card>
          <CardHeader>
            <CardTitle className="text-base">{t("Edit Tenant")}</CardTitle>
            <CardDescription>{t("Update tenant by editing YAML directly.")}</CardDescription>
          </CardHeader>
          <CardContent>
            {tenantYamlLoading ? (
              <div className="flex items-center gap-2 text-sm text-muted-foreground">
                <Spinner className="size-4" />
                {t("Loading tenant YAML...")}
              </div>
            ) : (
              <form onSubmit={handleUpdateTenantYaml} className="space-y-4">
                <div className="flex justify-end gap-2">
                  <Button type="button" variant="outline" size="sm" onClick={handleCopyYaml}>
                    <RiFileCopyLine className="mr-1 size-4" />
                    {t("Copy YAML")}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => {
                      if (isYamlEditable) {
                        setTenantYaml(tenantYamlSnapshot)
                      }
                      setIsYamlEditable((v) => !v)
                    }}
                  >
                    {isYamlEditable ? t("Cancel Edit") : t("Edit")}
                  </Button>
                </div>
                <div className="space-y-2">
                  <Label htmlFor="tenant-yaml-editor">{t("YAML Content")}</Label>
                  <textarea
                    id="tenant-yaml-editor"
                    value={tenantYaml}
                    onChange={(e) => setTenantYaml(e.target.value)}
                    readOnly={!isYamlEditable}
                    className={`dark:bg-input/30 border-input focus-visible:border-ring focus-visible:ring-ring/50 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive dark:aria-invalid:border-destructive/50 min-h-[460px] w-full rounded-none border px-2.5 py-2 font-mono text-xs transition-colors placeholder:text-muted-foreground focus-visible:ring-1 md:text-xs outline-none ${
                      isYamlEditable ? "bg-transparent" : "bg-muted/30 cursor-default"
                    }`}
                    spellCheck={false}
                  />
                </div>
                {isYamlEditable && (
                  <div className="flex gap-2">
                    <Button type="submit" disabled={editLoading}>
                      {editLoading && <Spinner className="mr-2 size-4" />}
                      {editLoading ? t("Saving...") : t("Save")}
                    </Button>
                    <Button type="button" variant="outline" onClick={loadTenantYaml} disabled={tenantYamlLoading}>
                      {tenantYamlLoading && <Spinner className="mr-2 size-4" />}
                      {t("Reload YAML")}
                    </Button>
                  </div>
                )}
              </form>
            )}
          </CardContent>
        </Card>
      )}

      {tab === "pools" && (
        <div className="space-y-4">
          <Card>
            <CardHeader className="pb-2">
              <CardDescription>
                {t(
                  "All pools in this tenant form one unified cluster. Data is distributed across all pools (erasure-coded); every pool is in use. To see disk usage per pool, use RustFS Console (S3 API port 9001) or check PVC usage in the cluster (e.g. kubectl).",
                )}
              </CardDescription>
            </CardHeader>
          </Card>
          <div className="flex justify-end">
            <Button size="sm" onClick={() => setAddPoolOpen(true)}>
              <RiAddLine className="mr-1 size-4" />
              {t("Add Pool")}
            </Button>
          </div>
          {addPoolOpen && (
            <Card>
              <CardHeader>
                <CardTitle className="text-base">{t("Add Pool")}</CardTitle>
                <CardDescription>{t("New pool will expand the unified cluster.")}</CardDescription>
              </CardHeader>
              <CardContent>
                <form onSubmit={handleAddPool} className="space-y-4">
                  <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-5">
                    <div className="space-y-2">
                      <Label>{t("Name")}</Label>
                      <Input
                        value={addPoolForm.name}
                        onChange={(e) => setAddPoolForm((f) => ({ ...f, name: e.target.value }))}
                      />
                    </div>
                    <div className="space-y-2">
                      <Label>{t("Servers")}</Label>
                      <Input
                        type="number"
                        min={1}
                        value={addPoolForm.servers}
                        onChange={(e) => setAddPoolForm((f) => ({ ...f, servers: parseInt(e.target.value, 10) || 0 }))}
                      />
                    </div>
                    <div className="space-y-2">
                      <Label>{t("Volumes per Server")}</Label>
                      <Input
                        type="number"
                        min={1}
                        value={addPoolForm.volumesPerServer}
                        onChange={(e) =>
                          setAddPoolForm((f) => ({
                            ...f,
                            volumesPerServer: parseInt(e.target.value, 10) || 0,
                          }))
                        }
                      />
                    </div>
                    <div className="space-y-2">
                      <Label>{t("Storage Size")}</Label>
                      <Input
                        value={addPoolForm.storageSize}
                        onChange={(e) => setAddPoolForm((f) => ({ ...f, storageSize: e.target.value }))}
                      />
                    </div>
                    <div className="space-y-2">
                      <Label>{t("Storage Class")}</Label>
                      <Input
                        value={addPoolForm.storageClass || ""}
                        onChange={(e) => setAddPoolForm((f) => ({ ...f, storageClass: e.target.value }))}
                      />
                    </div>
                  </div>
                  <div className="flex gap-2">
                    <Button type="submit" disabled={addPoolLoading}>
                      {addPoolLoading && <Spinner className="mr-2 size-4" />}
                      {t("Add Pool")}
                    </Button>
                    <Button type="button" variant="outline" onClick={() => setAddPoolOpen(false)}>
                      {t("Cancel")}
                    </Button>
                  </div>
                </form>
              </CardContent>
            </Card>
          )}
          <div className="rounded-md border border-border">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{t("Name")}</TableHead>
                  <TableHead>{t("Servers")}</TableHead>
                  <TableHead>{t("Volumes/Server")}</TableHead>
                  <TableHead>{t("State")}</TableHead>
                  <TableHead>{t("Ready")}</TableHead>
                  <TableHead className="w-[80px]">{t("Actions")}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {pools.map((p) => (
                  <TableRow key={p.name}>
                    <TableCell className="font-medium">{p.name}</TableCell>
                    <TableCell>{p.servers}</TableCell>
                    <TableCell>{p.volumes_per_server}</TableCell>
                    <TableCell>{p.state}</TableCell>
                    <TableCell>
                      {p.ready_replicas}/{p.replicas}
                    </TableCell>
                    <TableCell>
                      <Button
                        variant="ghost"
                        size="icon-xs"
                        className="text-destructive"
                        disabled={deletingPool === p.name}
                        onClick={() => handleDeletePool(p.name)}
                      >
                        {deletingPool === p.name ? (
                          <Spinner className="size-4" />
                        ) : (
                          <RiDeleteBinLine className="size-4" />
                        )}
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        </div>
      )}

      {tab === "pods" && (
        <div className="space-y-4">
          <div className="rounded-md border border-border">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{t("Name")}</TableHead>
                  <TableHead>{t("Pool")}</TableHead>
                  <TableHead>{t("Status")}</TableHead>
                  <TableHead>{t("Node")}</TableHead>
                  <TableHead>{t("Age")}</TableHead>
                  <TableHead className="w-[180px]">{t("Actions")}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {pods.map((p) => (
                  <TableRow key={p.name}>
                    <TableCell className="font-medium">{p.name}</TableCell>
                    <TableCell>{p.pool}</TableCell>
                    <TableCell>{p.status}</TableCell>
                    <TableCell>{p.node || "-"}</TableCell>
                    <TableCell>{p.age}</TableCell>
                    <TableCell>
                      <div className="flex gap-1">
                        <Button variant="ghost" size="icon-xs" title={t("Logs")} onClick={() => loadLogs(p.name)}>
                          <RiFileList3Line className="size-4" />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon-xs"
                          title={t("Restart")}
                          disabled={restartingPod === p.name}
                          onClick={() => handleRestartPod(p.name)}
                        >
                          {restartingPod === p.name ? (
                            <Spinner className="size-4" />
                          ) : (
                            <RiRestartLine className="size-4" />
                          )}
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon-xs"
                          className="text-destructive"
                          title={t("Delete")}
                          disabled={deletingPod === p.name}
                          onClick={() => handleDeletePod(p.name)}
                        >
                          {deletingPod === p.name ? (
                            <Spinner className="size-4" />
                          ) : (
                            <RiDeleteBinLine className="size-4" />
                          )}
                        </Button>
                      </div>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
          {logsPod && (
            <Card>
              <CardHeader className="flex flex-row items-center justify-between">
                <CardTitle className="text-base">
                  {t("Logs")}: {logsPod}
                </CardTitle>
                <Button variant="ghost" size="sm" onClick={() => setLogsPod(null)}>
                  {t("Close")}
                </Button>
              </CardHeader>
              <CardContent>
                {logsLoading ? (
                  <Spinner className="size-6" />
                ) : (
                  <pre className="max-h-96 overflow-auto rounded bg-muted p-4 text-xs font-mono whitespace-pre-wrap">
                    {logsContent}
                  </pre>
                )}
              </CardContent>
            </Card>
          )}
        </div>
      )}

      {tab === "encryption" && (
        <Card>
          <CardHeader>
            <CardTitle className="text-base">{t("Encryption")}</CardTitle>
            <CardDescription>
              {t("Configure server-side encryption (SSE) with a KMS backend. RustFS supports Local and Vault.")}
            </CardDescription>
          </CardHeader>
          <CardContent>
            {encLoading ? (
              <div className="flex items-center gap-2 text-sm text-muted-foreground">
                <Spinner className="size-4" />
                {t("Loading encryption config...")}
              </div>
            ) : (
              <form onSubmit={handleSaveEncryption} className="space-y-6">
                {/* Enable / Disable toggle */}
                <div className="flex items-center gap-3">
                  <label htmlFor="enc-toggle" className="text-sm font-medium">
                    {t("Enable Encryption")}
                  </label>
                  <input
                    id="enc-toggle"
                    type="checkbox"
                    checked={encEnabled}
                    onChange={(e) => setEncEnabled(e.target.checked)}
                    className="h-4 w-4 rounded border-border"
                  />
                </div>

                {encEnabled && (
                  <div className="space-y-6">
                    {/* Backend selector */}
                    <div className="space-y-2">
                      <Label>{t("KMS Backend")}</Label>
                      <div className="flex gap-4">
                        <label className="flex items-center gap-2 text-sm">
                          <input
                            type="radio"
                            name="enc-backend"
                            value="vault"
                            checked={encBackend === "vault"}
                            onChange={() => setEncBackend("vault")}
                          />
                          Vault
                        </label>
                        <label className="flex items-center gap-2 text-sm">
                          <input
                            type="radio"
                            name="enc-backend"
                            value="local"
                            checked={encBackend === "local"}
                            onChange={() => setEncBackend("local")}
                          />
                          Local
                        </label>
                      </div>
                    </div>

                    {/* Vault configuration */}
                    {encBackend === "vault" && (
                      <div className="space-y-4 rounded-md border border-border p-4">
                        <h4 className="text-sm font-semibold">{t("Vault Configuration")}</h4>
                        <div className="grid gap-4 sm:grid-cols-2">
                          <div className="space-y-2">
                            <Label>{t("Endpoint")}*</Label>
                            <Input
                              required
                              placeholder="https://vault.example.com:8200"
                              value={encVault.endpoint}
                              onChange={(e) => setEncVault((v) => ({ ...v, endpoint: e.target.value }))}
                            />
                          </div>
                          <div className="space-y-2">
                            <Label>{t("Engine")}</Label>
                            <Input
                              placeholder="kv"
                              value={encVault.engine}
                              onChange={(e) => setEncVault((v) => ({ ...v, engine: e.target.value }))}
                            />
                          </div>
                          <div className="space-y-2">
                            <Label>{t("Namespace")}</Label>
                            <Input
                              placeholder={`${t("Optional")} – Vault Enterprise`}
                              value={encVault.namespace}
                              onChange={(e) => setEncVault((v) => ({ ...v, namespace: e.target.value }))}
                            />
                          </div>
                          <div className="space-y-2">
                            <Label>{t("Prefix")}</Label>
                            <Input
                              placeholder={t("Optional")}
                              value={encVault.prefix}
                              onChange={(e) => setEncVault((v) => ({ ...v, prefix: e.target.value }))}
                            />
                          </div>
                        </div>

                        {/* Auth type selector */}
                        <div className="space-y-2 pt-2">
                          <Label>{t("Auth Type")}</Label>
                          <div className="flex gap-4">
                            <label className="flex items-center gap-2 text-sm">
                              <input
                                type="radio"
                                name="vault-auth"
                                value="token"
                                checked={encVault.authType !== "approle"}
                                onChange={() => setEncVault((v) => ({ ...v, authType: "token" }))}
                              />
                              Token
                            </label>
                            <label className="flex items-center gap-2 text-sm">
                              <input
                                type="radio"
                                name="vault-auth"
                                value="approle"
                                checked={encVault.authType === "approle"}
                                onChange={() => setEncVault((v) => ({ ...v, authType: "approle" }))}
                              />
                              AppRole
                            </label>
                          </div>
                        </div>

                        {/* AppRole section */}
                        {encVault.authType === "approle" && (
                          <div className="space-y-4 rounded-md border border-dashed border-border p-4">
                            <div className="flex items-center gap-2">
                              <h5 className="text-sm font-semibold">App Role</h5>
                              <span className="rounded bg-yellow-100 px-1.5 py-0.5 text-[10px] font-medium text-yellow-800 dark:bg-yellow-900/30 dark:text-yellow-300">
                                {t("Not yet implemented in backend")}
                              </span>
                            </div>
                            <div className="grid gap-4 sm:grid-cols-2">
                              <div className="space-y-2">
                                <Label>{t("Engine")}</Label>
                                <Input
                                  placeholder="approle"
                                  value={encAppRole.engine}
                                  onChange={(e) => setEncAppRole((a) => ({ ...a, engine: e.target.value }))}
                                />
                              </div>
                              <div className="space-y-2">
                                <Label>{t("Retry (Seconds)")}</Label>
                                <Input
                                  type="number"
                                  placeholder="10"
                                  value={encAppRole.retrySeconds}
                                  onChange={(e) => setEncAppRole((a) => ({ ...a, retrySeconds: e.target.value }))}
                                />
                              </div>
                            </div>
                            <p className="text-xs text-muted-foreground">
                              {t(
                                "AppRole ID and Secret are stored in the KMS Secret (keys: vault-approle-id, vault-approle-secret).",
                              )}
                            </p>
                          </div>
                        )}
                      </div>
                    )}

                    {/* Local configuration */}
                    {encBackend === "local" && (
                      <div className="space-y-4 rounded-md border border-border p-4">
                        <h4 className="text-sm font-semibold">{t("Local KMS Configuration")}</h4>
                        <div className="grid gap-4 sm:grid-cols-2">
                          <div className="space-y-2">
                            <Label>{t("Key Directory")}</Label>
                            <Input
                              placeholder="/data/kms-keys"
                              value={encLocal.keyDirectory}
                              onChange={(e) => setEncLocal((l) => ({ ...l, keyDirectory: e.target.value }))}
                            />
                          </div>
                          <div className="space-y-2">
                            <Label>{t("Master Key ID")}</Label>
                            <Input
                              placeholder="default-master-key"
                              value={encLocal.masterKeyId}
                              onChange={(e) => setEncLocal((l) => ({ ...l, masterKeyId: e.target.value }))}
                            />
                          </div>
                        </div>
                      </div>
                    )}

                    {/* Status — Ping is mainly useful for remote backends (Vault) */}
                    {encBackend === "vault" && (
                      <div className="space-y-4 rounded-md border border-border p-4">
                        <h4 className="text-sm font-semibold">{t("Status")}</h4>
                        <div className="grid gap-4 sm:grid-cols-2">
                          <div className="space-y-2">
                            <Label>{t("Ping (Seconds)")}</Label>
                            <Input
                              type="number"
                              placeholder={t("Optional")}
                              value={encPingSeconds}
                              onChange={(e) => setEncPingSeconds(e.target.value)}
                            />
                            <p className="text-xs text-muted-foreground">
                              {t("Health check interval for KMS connectivity.")}
                            </p>
                          </div>
                        </div>
                      </div>
                    )}

                    {/* KMS Secret name */}
                    <div className="space-y-2">
                      <Label>{t("KMS Secret Name")}</Label>
                      <Input
                        placeholder={`${t("Optional")} – ${t("Secret containing vault-token")}`}
                        value={encKmsSecretName}
                        onChange={(e) => setEncKmsSecretName(e.target.value)}
                      />
                      <p className="text-xs text-muted-foreground">
                        {encBackend === "vault"
                          ? encVault.authType === "approle"
                            ? t("Secret must contain 'vault-approle-id' and 'vault-approle-secret'.")
                            : t("Secret must contain key 'vault-token'.")
                          : t("Not required for Local backend.")}
                      </p>
                    </div>
                  </div>
                )}

                {/* Save button */}
                <div className="flex gap-2">
                  <Button type="submit" disabled={encSaving}>
                    {encSaving && <Spinner className="mr-2 size-4" />}
                    {encSaving ? t("Saving...") : t("Save")}
                  </Button>
                  <Button type="button" variant="outline" disabled={encLoading} onClick={loadEncryption}>
                    {encLoading && <Spinner className="mr-2 size-4" />}
                    {t("Reload")}
                  </Button>
                </div>
              </form>
            )}
          </CardContent>
        </Card>
      )}

      {tab === "security" && (
        <Card>
          <CardHeader>
            <CardTitle className="text-base">{t("SecurityContext")}</CardTitle>
            <CardDescription>
              {t(
                "Override Pod SecurityContext for RustFS pods (runAsUser, runAsGroup, fsGroup). Changes apply after Pods are recreated.",
              )}
            </CardDescription>
          </CardHeader>
          <CardContent>
            {secCtxLoading ? (
              <div className="flex gap-2 text-sm text-muted-foreground">
                <Spinner className="size-4" />
                {t("Loading...")}
              </div>
            ) : (
              <form onSubmit={handleSaveSecurityContext} className="space-y-6">
                <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
                  <div className="space-y-2">
                    <Label>{t("Run As User")}</Label>
                    <Input
                      type="number"
                      placeholder="10001"
                      value={secCtx.runAsUser}
                      onChange={(e) => setSecCtx((s) => ({ ...s, runAsUser: e.target.value }))}
                    />
                  </div>
                  <div className="space-y-2">
                    <Label>{t("Run As Group")}</Label>
                    <Input
                      type="number"
                      placeholder="10001"
                      value={secCtx.runAsGroup}
                      onChange={(e) => setSecCtx((s) => ({ ...s, runAsGroup: e.target.value }))}
                    />
                  </div>
                  <div className="space-y-2">
                    <Label>{t("FsGroup")}</Label>
                    <Input
                      type="number"
                      placeholder="10001"
                      value={secCtx.fsGroup}
                      onChange={(e) => setSecCtx((s) => ({ ...s, fsGroup: e.target.value }))}
                    />
                  </div>
                  <div className="flex items-end gap-3 pb-2">
                    <label htmlFor="sec-nonroot" className="text-sm whitespace-nowrap">
                      {t("Do not run as Root")}
                    </label>
                    <input
                      id="sec-nonroot"
                      type="checkbox"
                      checked={secCtx.runAsNonRoot}
                      onChange={(e) => setSecCtx((s) => ({ ...s, runAsNonRoot: e.target.checked }))}
                      className="h-4 w-4 rounded border-border"
                    />
                  </div>
                </div>
                <div className="flex gap-2">
                  <Button type="submit" disabled={secCtxSaving}>
                    {secCtxSaving && <Spinner className="mr-2 size-4" />}
                    {secCtxSaving ? t("Saving...") : t("Save")}
                  </Button>
                  <Button type="button" variant="outline" disabled={secCtxLoading} onClick={loadSecurityContext}>
                    {secCtxLoading && <Spinner className="mr-2 size-4" />}
                    {t("Reload")}
                  </Button>
                </div>
              </form>
            )}
          </CardContent>
        </Card>
      )}

      {tab === "events" && (
        <div className="rounded-md border border-border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t("Type")}</TableHead>
                <TableHead>{t("Reason")}</TableHead>
                <TableHead>{t("Message")}</TableHead>
                <TableHead>{t("Object")}</TableHead>
                <TableHead>{t("Last")}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {eventsLoading && events.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={5} className="text-center text-muted-foreground py-8">
                    <Spinner className="inline size-4" />
                  </TableCell>
                </TableRow>
              ) : events.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={5} className="text-center text-muted-foreground py-8">
                    {t("No events")}
                  </TableCell>
                </TableRow>
              ) : (
                events.map((ev, i) => (
                  <TableRow key={i}>
                    <TableCell>{ev.event_type}</TableCell>
                    <TableCell>{ev.reason}</TableCell>
                    <TableCell className="max-w-md truncate">{ev.message}</TableCell>
                    <TableCell>{ev.involved_object}</TableCell>
                    <TableCell className="text-muted-foreground">{ev.last_timestamp || "-"}</TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        </div>
      )}
    </Page>
  )
}
