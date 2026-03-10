"use client"

import { useEffect, useMemo, useRef, useState } from "react"
import Link from "next/link"
import { useTranslation } from "react-i18next"
import { toast } from "sonner"
import { RiAddLine, RiDeleteBinLine, RiEyeLine, RiSearchLine } from "@remixicon/react"
import { Page } from "@/components/page"
import { PageHeader } from "@/components/page-header"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Spinner } from "@/components/ui/spinner"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import * as api from "@/lib/api"
import { ApiError } from "@/lib/api-client"
import { routes } from "@/lib/routes"
import type { ServiceInfo, TenantListItem } from "@/types/api"

const ALL_NAMESPACES = "__all__"

interface TenantMeta {
  replicas: number | null
  version: string
  capacity: string
  endpoint: string
}

function makeTenantKey(namespace: string, name: string): string {
  return `${namespace}/${name}`
}

function parseSizeToBytes(size: string | null): number | null {
  if (!size) return null
  const match = size.trim().match(/^(\d+(?:\.\d+)?)\s*([kmgtpe]?i?b?)?$/i)
  if (!match) return null

  const value = Number.parseFloat(match[1] ?? "0")
  if (!Number.isFinite(value) || value < 0) return null

  const rawUnit = (match[2] ?? "").toUpperCase().replace(/B$/, "")
  if (!rawUnit) return value

  const binary = rawUnit.endsWith("I")
  const unit = binary ? rawUnit.slice(0, -1) : rawUnit
  const powers: Record<string, number> = {
    "": 0,
    K: 1,
    M: 2,
    G: 3,
    T: 4,
    P: 5,
    E: 6,
  }
  const power = powers[unit]
  if (power == null) return null
  const base = binary ? 1024 : 1000
  return value * base ** power
}

function formatBinaryBytes(bytes: number): string {
  const tebibyte = 1024 ** 4
  const gibibyte = 1024 ** 3
  const mebibyte = 1024 ** 2

  const format = (value: number) => {
    if (Number.isInteger(value)) return String(value)
    return value.toFixed(1).replace(/\.0$/, "")
  }

  if (bytes >= tebibyte) return `${format(bytes / tebibyte)} TiB`
  if (bytes >= gibibyte) return `${format(bytes / gibibyte)} GiB`
  if (bytes >= mebibyte) return `${format(bytes / mebibyte)} MiB`
  return `${format(bytes)} B`
}

function getVersionFromImage(image: string | null): string {
  if (!image) return "-"
  const index = image.lastIndexOf(":")
  if (index === -1 || index === image.length - 1) return image
  return image.slice(index + 1)
}

function buildEndpoint(namespace: string, services: ServiceInfo[]): string {
  if (services.length === 0) return "-"
  const service = services[0]
  if (!service) return "-"
  const firstPort = service.ports[0]?.port
  if (firstPort == null) return `${service.name}.${namespace}.svc`
  const protocol = firstPort === 443 ? "https" : "http"
  return `${protocol}://${service.name}.${namespace}.svc:${firstPort}`
}

function statusBadgeClass(state: string): string {
  const normalized = state.toLowerCase()
  if (normalized.includes("run")) return "bg-emerald-50 text-emerald-700 border-emerald-200"
  if (normalized.includes("error") || normalized.includes("fail")) return "bg-red-50 text-red-700 border-red-200"
  if (normalized.includes("stop")) return "bg-zinc-100 text-zinc-700 border-zinc-200"
  if (normalized.includes("provision")) return "bg-blue-50 text-blue-700 border-blue-200"
  return "bg-amber-50 text-amber-700 border-amber-200"
}

export default function TenantsListPage() {
  const { t } = useTranslation()
  const [tenants, setTenants] = useState<TenantListItem[]>([])
  const [tenantMeta, setTenantMeta] = useState<Record<string, TenantMeta>>({})
  const [namespaces, setNamespaces] = useState<string[]>([])
  const [selectedNamespace, setSelectedNamespace] = useState<string>(ALL_NAMESPACES)
  const [searchText, setSearchText] = useState("")
  const [loading, setLoading] = useState(true)
  const [metaLoading, setMetaLoading] = useState(false)
  const [deleting, setDeleting] = useState<string | null>(null)
  const loadSeq = useRef(0)

  const loadNamespaces = async () => {
    try {
      const res = await api.listNamespaces()
      setNamespaces(Array.from(new Set(res.namespaces.map((item) => item.name))).sort())
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Failed to load namespaces"))
    }
  }

  const load = async (namespace: string) => {
    setLoading(true)
    const currentSeq = ++loadSeq.current

    try {
      const res =
        namespace === ALL_NAMESPACES ? await api.listTenants() : await api.listTenantsByNamespace(namespace)
      if (currentSeq !== loadSeq.current) return
      setTenants(res.tenants)

      if (res.tenants.length === 0) {
        setTenantMeta({})
        return
      }

      setMetaLoading(true)
      const metaEntries = await Promise.all(
        res.tenants.map(async (tnt) => {
          const key = makeTenantKey(tnt.namespace, tnt.name)
          try {
            const [detailRes, poolRes] = await Promise.all([api.getTenant(tnt.namespace, tnt.name), api.listPools(tnt.namespace, tnt.name)])
            const replicas = poolRes.pools.reduce((sum, pool) => sum + pool.replicas, 0)
            const capacityBytes = poolRes.pools.reduce((sum, pool) => {
              const oneVolume = parseSizeToBytes(pool.volume_size)
              if (oneVolume == null) return sum
              return sum + oneVolume * pool.total_volumes
            }, 0)
            return [
              key,
              {
                replicas: replicas || null,
                version: getVersionFromImage(detailRes.image),
                capacity: capacityBytes > 0 ? formatBinaryBytes(capacityBytes) : "-",
                endpoint: buildEndpoint(tnt.namespace, detailRes.services),
              },
            ] as const
          } catch {
            return [
              key,
              {
                replicas: tnt.pools.reduce((sum, pool) => sum + pool.servers, 0) || null,
                version: "-",
                capacity: "-",
                endpoint: "-",
              },
            ] as const
          }
        }),
      )

      if (currentSeq !== loadSeq.current) return
      setTenantMeta(Object.fromEntries(metaEntries))
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Failed to load tenants"))
    } finally {
      if (currentSeq === loadSeq.current) {
        setLoading(false)
        setMetaLoading(false)
      }
    }
  }

  useEffect(() => {
    loadNamespaces()
  }, []) // eslint-disable-line react-hooks/exhaustive-deps -- run once on mount

  useEffect(() => {
    load(selectedNamespace)
  }, [selectedNamespace]) // eslint-disable-line react-hooks/exhaustive-deps -- reload when namespace filter changes

  const filteredTenants = useMemo(() => {
    const keyword = searchText.trim().toLowerCase()
    if (!keyword) return tenants

    return tenants.filter((tenant) => {
      const key = makeTenantKey(tenant.namespace, tenant.name)
      const meta = tenantMeta[key]
      return [
        tenant.name,
        tenant.namespace,
        tenant.state,
        meta?.version ?? "",
        meta?.endpoint ?? "",
        meta?.capacity ?? "",
      ]
        .join(" ")
        .toLowerCase()
        .includes(keyword)
    })
  }, [searchText, tenantMeta, tenants])

  const summary = useMemo(() => {
    let running = 0
    let abnormal = 0
    let stopped = 0

    for (const item of filteredTenants) {
      const state = item.state.toLowerCase()
      if (state.includes("run")) running += 1
      else if (state.includes("error") || state.includes("fail")) abnormal += 1
      else if (state.includes("stop")) stopped += 1
    }

    return {
      total: filteredTenants.length,
      running,
      abnormal,
      stopped,
    }
  }, [filteredTenants])

  const namespaceOptions = useMemo(() => {
    const fromTenants = tenants.map((item) => item.namespace)
    const merged = Array.from(new Set([...namespaces, ...fromTenants]))
    return merged.sort()
  }, [namespaces, tenants])

  const handleDelete = async (namespace: string, name: string) => {
    if (!confirm(t('Delete tenant "{{name}}"? This cannot be undone.', { name }))) return
    setDeleting(`${namespace}/${name}`)
    try {
      await api.deleteTenant(namespace, name)
      toast.success(t("Tenant deleted"))
      load(selectedNamespace)
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Delete failed"))
    } finally {
      setDeleting(null)
    }
  }

  return (
    <Page>
      <PageHeader
        actions={
          <Button asChild size="sm">
            <Link href={routes.tenantNew} prefetch={false}>
              <RiAddLine className="mr-1 size-4" />
              {t("Create Tenant")}
            </Link>
          </Button>
        }
      >
        <h1 className="text-lg font-semibold">{t("Tenants")}</h1>
        <p className="text-sm text-muted-foreground">{t("Manage RustFS tenant instances.")}</p>
      </PageHeader>

      <div className="mt-4 flex flex-wrap gap-2">
        <span className="inline-flex items-center gap-1 rounded border border-border bg-muted/40 px-2 py-1 text-xs">
          <span className="font-medium">{t("Total")}</span> {summary.total}
        </span>
        <span className="inline-flex items-center gap-1 rounded border border-emerald-200 bg-emerald-50 px-2 py-1 text-xs text-emerald-700">
          <span className="size-1.5 rounded-full bg-emerald-500" />
          <span className="font-medium">{t("Running")}</span> {summary.running}
        </span>
        <span className="inline-flex items-center gap-1 rounded border border-red-200 bg-red-50 px-2 py-1 text-xs text-red-700">
          <span className="size-1.5 rounded-full bg-red-500" />
          <span className="font-medium">{t("Abnormal")}</span> {summary.abnormal}
        </span>
        <span className="inline-flex items-center gap-1 rounded border border-zinc-200 bg-zinc-100 px-2 py-1 text-xs text-zinc-700">
          <span className="size-1.5 rounded-full bg-zinc-500" />
          <span className="font-medium">{t("Stopped")}</span> {summary.stopped}
        </span>
      </div>

      <div className="mt-4 flex flex-col gap-2 lg:flex-row lg:items-center">
        <div className="relative flex-1">
          <RiSearchLine className="pointer-events-none absolute left-2 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={searchText}
            onChange={(e) => setSearchText(e.target.value)}
            placeholder={t("Search by name, namespace or endpoint...")}
            className="pl-8"
          />
        </div>
        <select
          value={selectedNamespace}
          onChange={(e) => setSelectedNamespace(e.target.value)}
          className="dark:bg-input/30 border-input h-8 min-w-[220px] rounded-none border bg-transparent px-2.5 text-xs outline-none"
        >
          <option value={ALL_NAMESPACES}>{t("All Namespaces")}</option>
          {namespaceOptions.map((namespace) => (
            <option key={namespace} value={namespace}>
              {namespace}
            </option>
          ))}
        </select>
        <Button asChild variant="outline" size="sm">
          <Link href={routes.cluster} prefetch={false}>
            {t("Manage Namespaces")}
          </Link>
        </Button>
      </div>

      {loading ? (
        <div className="flex items-center justify-center py-12">
          <Spinner className="size-8" />
        </div>
      ) : filteredTenants.length === 0 ? (
        <div className="mt-4 rounded-lg border border-dashed border-border py-12 text-center text-sm text-muted-foreground">
          {searchText ? t("No tenants match the current filters.") : t("No tenants yet. Create one to get started.")}
          <div className="mt-4">
            <Button asChild size="sm">
              <Link href={routes.tenantNew} prefetch={false}>
                {t("Create Tenant")}
              </Link>
            </Button>
          </div>
        </div>
      ) : (
        <div className="mt-4 rounded-md border border-border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t("Name")}</TableHead>
                <TableHead>{t("Namespace")}</TableHead>
                <TableHead>{t("Status")}</TableHead>
                <TableHead>{t("Replicas")}</TableHead>
                <TableHead>{t("Version")}</TableHead>
                <TableHead>{t("Total Capacity")}</TableHead>
                <TableHead>{t("Endpoint")}</TableHead>
                <TableHead>{t("Created")}</TableHead>
                <TableHead className="w-[120px]">{t("Actions")}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {filteredTenants.map((tenant) => {
                const key = makeTenantKey(tenant.namespace, tenant.name)
                const meta = tenantMeta[key]
                return (
                  <TableRow key={key}>
                    <TableCell className="font-medium">
                      <Link
                        href={routes.tenantDetail(tenant.namespace, tenant.name)}
                        prefetch={false}
                        className="text-primary hover:underline"
                      >
                        {tenant.name}
                      </Link>
                    </TableCell>
                    <TableCell>{tenant.namespace}</TableCell>
                    <TableCell>
                      <span className={`inline-flex rounded border px-2 py-0.5 text-xs ${statusBadgeClass(tenant.state)}`}>
                        {tenant.state}
                      </span>
                    </TableCell>
                    <TableCell>{meta?.replicas ?? "-"}</TableCell>
                    <TableCell className="text-muted-foreground">{meta?.version ?? "-"}</TableCell>
                    <TableCell>{meta?.capacity ?? "-"}</TableCell>
                    <TableCell className="max-w-[340px] truncate text-muted-foreground" title={meta?.endpoint ?? "-"}>
                      {meta?.endpoint ?? "-"}
                    </TableCell>
                    <TableCell className="text-muted-foreground">
                      {tenant.created_at ? new Date(tenant.created_at).toLocaleDateString() : "-"}
                    </TableCell>
                    <TableCell>
                      <div className="flex gap-1">
                        <Link
                          href={routes.tenantDetail(tenant.namespace, tenant.name)}
                          prefetch={false}
                          title={t("View")}
                          className="inline-flex size-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-accent-foreground"
                        >
                          <RiEyeLine className="size-4" />
                        </Link>
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          className="text-destructive hover:text-destructive"
                          disabled={deleting === key}
                          onClick={() => handleDelete(tenant.namespace, tenant.name)}
                          title={t("Delete")}
                        >
                          {deleting === key ? <Spinner className="size-4" /> : <RiDeleteBinLine className="size-4" />}
                        </Button>
                      </div>
                    </TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
          {metaLoading && (
            <div className="flex items-center gap-2 border-t border-border px-3 py-2 text-xs text-muted-foreground">
              <Spinner className="size-3.5" />
              {t("Loading tenant metrics...")}
            </div>
          )}
        </div>
      )}
    </Page>
  )
}
