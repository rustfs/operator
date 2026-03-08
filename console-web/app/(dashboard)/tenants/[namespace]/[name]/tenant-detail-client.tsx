"use client"

import { useRouter } from "next/navigation"
import Link from "next/link"
import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { toast } from "sonner"
import { RiArrowLeftLine, RiDeleteBinLine, RiAddLine, RiFileList3Line, RiRestartLine } from "@remixicon/react"
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
  AddPoolRequest,
  UpdateTenantRequest,
} from "@/types/api"
import { ApiError } from "@/lib/api-client"

type Tab = "overview" | "edit" | "pools" | "pods" | "events"

interface TenantDetailClientProps {
  namespace: string
  name: string
}

export function TenantDetailClient({ namespace, name }: TenantDetailClientProps) {
  const router = useRouter()
  const { t } = useTranslation()

  const [tab, setTab] = useState<Tab>("overview")
  const [tenant, setTenant] = useState<TenantDetailsResponse | null>(null)
  const [pools, setPools] = useState<PoolDetails[]>([])
  const [pods, setPods] = useState<PodListItem[]>([])
  const [events, setEvents] = useState<EventItem[]>([])
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
  const [editForm, setEditForm] = useState<UpdateTenantRequest>({})
  const [editLoading, setEditLoading] = useState(false)

  const loadTenant = async () => {
    const [detailResult, poolResult, podResult, eventResult] = await Promise.allSettled([
      api.getTenant(namespace, name),
      api.listPools(namespace, name),
      api.listPods(namespace, name),
      api.listTenantEvents(namespace, name),
    ])

    const detailOk = detailResult.status === "fulfilled"
    const poolOk = poolResult.status === "fulfilled"
    const podOk = podResult.status === "fulfilled"
    const eventOk = eventResult.status === "fulfilled"

    if (detailOk && poolOk && podOk) {
      setTenant(detailResult.value)
      setPools(poolResult.value.pools)
      setPods(podResult.value.pods)
      setEvents(eventOk ? eventResult.value.events : [])
      if (!eventOk) {
        toast.error(t("Events could not be loaded"))
      }
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

  useEffect(() => {
    loadTenant()
  }, [namespace, name]) // eslint-disable-line react-hooks/exhaustive-deps -- reload when route params change

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

  const handleUpdateTenant = async (e: React.FormEvent) => {
    e.preventDefault()
    const body: UpdateTenantRequest = {}
    if (editForm.image !== undefined) body.image = editForm.image || undefined
    if (editForm.mount_path !== undefined) body.mount_path = editForm.mount_path || undefined
    if (editForm.creds_secret !== undefined) body.creds_secret = editForm.creds_secret || undefined
    if (Object.keys(body).length === 0) {
      toast.warning(t("No changes to save"))
      return
    }
    setEditLoading(true)
    try {
      await api.updateTenant(namespace, name, body)
      toast.success(t("Tenant updated"))
      loadTenant()
      setEditForm({})
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Update failed"))
    } finally {
      setEditLoading(false)
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
    { id: "edit", labelKey: "Edit" },
    { id: "pools", labelKey: "Pools" },
    { id: "pods", labelKey: "Pods" },
    { id: "events", labelKey: "Events" },
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
            <CardDescription>{t("Update tenant image, mount path or credentials secret.")}</CardDescription>
          </CardHeader>
          <CardContent>
            <form onSubmit={handleUpdateTenant} className="space-y-4 max-w-md">
              <div className="space-y-2">
                <Label htmlFor="edit-image">{t("Image")}</Label>
                <Input
                  id="edit-image"
                  value={editForm.image ?? tenant.image ?? ""}
                  onChange={(e) => setEditForm((f) => ({ ...f, image: e.target.value }))}
                  placeholder="rustfs/rustfs:latest"
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="edit-mount">{t("Mount Path")}</Label>
                <Input
                  id="edit-mount"
                  value={editForm.mount_path ?? tenant.mount_path ?? ""}
                  onChange={(e) => setEditForm((f) => ({ ...f, mount_path: e.target.value }))}
                  placeholder="/data/rustfs"
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="edit-creds">{t("Credentials Secret")}</Label>
                <Input
                  id="edit-creds"
                  value={editForm.creds_secret ?? ""}
                  onChange={(e) => setEditForm((f) => ({ ...f, creds_secret: e.target.value }))}
                  placeholder=""
                />
              </div>
              <Button type="submit" disabled={editLoading}>
                {editLoading && <Spinner className="mr-2 size-4" />}
                {editLoading ? t("Saving...") : t("Save")}
              </Button>
            </form>
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
              {events.length === 0 ? (
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
