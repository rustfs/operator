"use client"

import { useEffect, useMemo, useRef, useState } from "react"
import { useRouter } from "next/navigation"
import Link from "next/link"
import { useTranslation } from "react-i18next"
import { toast } from "sonner"
import {
  RiAddLine,
  RiDeleteBinLine,
  RiExternalLinkLine,
  RiEyeLine,
  RiFileCopyLine,
  RiMore2Line,
  RiPencilLine,
  RiSearchLine,
} from "@remixicon/react"
import { Page } from "@/components/page"
import { PageHeader } from "@/components/page-header"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Input } from "@/components/ui/input"
import { Spinner } from "@/components/ui/spinner"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import * as api from "@/lib/api"
import { ApiError } from "@/lib/api-client"
import { routes } from "@/lib/routes"
import { normalizeTenantLifecycleState } from "@/lib/tenant-state"
import { parseSizeToBytes, formatBinaryBytes } from "@/lib/utils"
import type { ServiceInfo, TenantLifecycleState, TenantListItem, TenantStateCountsResponse } from "@/types/api"

const ALL_NAMESPACES = "__all__"
const TENANT_STATES: TenantLifecycleState[] = ["Ready", "Reconciling", "Blocked", "Degraded", "NotReady", "Unknown"]
const EMPTY_STATE_COUNTS: Record<TenantLifecycleState, number> = {
  Ready: 0,
  Reconciling: 0,
  Blocked: 0,
  Updating: 0,
  Degraded: 0,
  NotReady: 0,
  Unknown: 0,
}

const STATE_THEME: Record<
  TenantLifecycleState,
  {
    badge: string
    dot: string
    label: string
    activeCard: string
  }
> = {
  Ready: {
    badge: "bg-emerald-50 text-emerald-700 border-emerald-200",
    dot: "bg-emerald-500",
    label: "text-emerald-700",
    activeCard: "border-emerald-300 ring-1 ring-emerald-200",
  },
  Reconciling: {
    badge: "bg-blue-50 text-blue-700 border-blue-200",
    dot: "bg-blue-500",
    label: "text-blue-700",
    activeCard: "border-blue-300 ring-1 ring-blue-200",
  },
  Blocked: {
    badge: "bg-purple-50 text-purple-700 border-purple-200",
    dot: "bg-purple-500",
    label: "text-purple-700",
    activeCard: "border-purple-300 ring-1 ring-purple-200",
  },
  Updating: {
    badge: "bg-blue-50 text-blue-700 border-blue-200",
    dot: "bg-blue-500",
    label: "text-blue-700",
    activeCard: "border-blue-300 ring-1 ring-blue-200",
  },
  Degraded: {
    badge: "bg-amber-50 text-amber-700 border-amber-200",
    dot: "bg-amber-500",
    label: "text-amber-700",
    activeCard: "border-amber-300 ring-1 ring-amber-200",
  },
  NotReady: {
    badge: "bg-red-50 text-red-700 border-red-200",
    dot: "bg-red-500",
    label: "text-red-700",
    activeCard: "border-red-300 ring-1 ring-red-200",
  },
  Unknown: {
    badge: "bg-zinc-100 text-zinc-700 border-zinc-200",
    dot: "bg-zinc-500",
    label: "text-zinc-700",
    activeCard: "border-zinc-300 ring-1 ring-zinc-200",
  },
}

interface TenantMeta {
  replicas: number | null
  version: string
  capacity: string
  endpoint: string
}

function makeTenantKey(namespace: string, name: string): string {
  return `${namespace}/${name}`
}

const normalizeTenantState = normalizeTenantLifecycleState

function parseStateCounts(payload: TenantStateCountsResponse): Record<TenantLifecycleState, number> {
  const result: Record<TenantLifecycleState, number> = { ...EMPTY_STATE_COUNTS }
  const append = (state: string, count: number) => {
    if (!Number.isFinite(count) || count < 0) return
    const normalizedState = normalizeTenantState(state)
    result[normalizedState] += Math.trunc(count)
  }

  if (Array.isArray(payload)) {
    for (const item of payload) {
      if (item && typeof item === "object" && typeof item.state === "string" && typeof item.count === "number") {
        append(item.state, item.count)
      }
    }
    return result
  }

  if (typeof payload === "object" && payload != null) {
    const obj = payload as Record<string, unknown>
    const countsMap =
      obj.counts && typeof obj.counts === "object" && !Array.isArray(obj.counts)
        ? (obj.counts as Record<string, unknown>)
        : null
    if (countsMap) {
      for (const [state, value] of Object.entries(countsMap)) {
        if (typeof value === "number") append(state, value)
      }
      return result
    }

    const listLike = Array.isArray(obj.state_counts) ? obj.state_counts : Array.isArray(obj.counts) ? obj.counts : null

    if (listLike) {
      for (const item of listLike) {
        if (item && typeof item === "object") {
          const row = item as Record<string, unknown>
          if (typeof row.state === "string" && typeof row.count === "number") {
            append(row.state, row.count)
          }
        }
      }
      return result
    }

    for (const [state, value] of Object.entries(obj)) {
      if (state === "total") continue
      if (typeof value === "number") append(state, value)
    }
  }

  return result
}

function extractTotal(payload: TenantStateCountsResponse): number {
  if (typeof payload === "object" && payload != null) {
    const obj = payload as Record<string, unknown>
    if (typeof obj.total === "number" && obj.total >= 0) {
      return Math.trunc(obj.total)
    }
  }
  return 0
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

export default function TenantsListPage() {
  const router = useRouter()
  const { t } = useTranslation()
  const [tenants, setTenants] = useState<TenantListItem[]>([])
  const [tenantMeta, setTenantMeta] = useState<Record<string, TenantMeta>>({})
  const [stateCounts, setStateCounts] = useState<Record<TenantLifecycleState, number>>({ ...EMPTY_STATE_COUNTS })
  const [totalCount, setTotalCount] = useState(0)
  const [namespaces, setNamespaces] = useState<string[]>([])
  const [selectedNamespace, setSelectedNamespace] = useState<string>(ALL_NAMESPACES)
  const [selectedState, setSelectedState] = useState<TenantLifecycleState | null>(null)
  const [searchText, setSearchText] = useState("")
  const [loading, setLoading] = useState(true)
  const [metaLoading, setMetaLoading] = useState(false)
  const [deleting, setDeleting] = useState<string | null>(null)
  const loadSeq = useRef(0)
  const countSeq = useRef(0)

  const loadNamespaces = async () => {
    try {
      const res = await api.listNamespaces()
      setNamespaces(Array.from(new Set(res.namespaces.map((item) => item.name))).sort())
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Failed to load namespaces"))
    }
  }

  const loadStateCounts = async (namespace: string) => {
    const currentSeq = ++countSeq.current
    try {
      const res =
        namespace === ALL_NAMESPACES
          ? await api.listTenantStateCounts()
          : await api.listTenantStateCountsByNamespace(namespace)
      if (currentSeq !== countSeq.current) return
      setStateCounts(parseStateCounts(res))
      setTotalCount(extractTotal(res))
    } catch (e) {
      if (currentSeq !== countSeq.current) return
      const err = e as ApiError
      toast.error(err.message || t("Failed to load tenant state counts"))
      setStateCounts({ ...EMPTY_STATE_COUNTS })
      setTotalCount(0)
    }
  }

  const load = async (namespace: string, state: TenantLifecycleState | null) => {
    setLoading(true)
    const currentSeq = ++loadSeq.current

    try {
      const params = state ? { state } : undefined
      const res =
        namespace === ALL_NAMESPACES
          ? await api.listTenants(params)
          : await api.listTenantsByNamespace(namespace, params)
      if (currentSeq !== loadSeq.current) return
      setTenants(res.tenants)

      if (res.tenants.length === 0) {
        setTenantMeta({})
        return
      }

      setMetaLoading(true)
      const metaEntries = await Promise.all(
        res.tenants.map(async (tenant) => {
          const key = makeTenantKey(tenant.namespace, tenant.name)
          try {
            const [detailRes, poolRes] = await Promise.all([
              api.getTenant(tenant.namespace, tenant.name),
              api.listPools(tenant.namespace, tenant.name),
            ])
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
                endpoint: buildEndpoint(tenant.namespace, detailRes.services),
              },
            ] as const
          } catch {
            return [
              key,
              {
                replicas: tenant.pools.reduce((sum, pool) => sum + pool.servers, 0) || null,
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
    loadStateCounts(selectedNamespace)
  }, [selectedNamespace]) // eslint-disable-line react-hooks/exhaustive-deps -- reload state counts when namespace changes

  useEffect(() => {
    load(selectedNamespace, selectedState)
  }, [selectedNamespace, selectedState]) // eslint-disable-line react-hooks/exhaustive-deps -- reload when filters change

  const filteredTenants = useMemo(() => {
    const keyword = searchText.trim().toLowerCase()
    if (!keyword) return tenants

    return tenants.filter((tenant) => {
      const key = makeTenantKey(tenant.namespace, tenant.name)
      const meta = tenantMeta[key]
      return [
        tenant.name,
        tenant.namespace,
        normalizeTenantState(tenant.state),
        meta?.version ?? "",
        meta?.endpoint ?? "",
        meta?.capacity ?? "",
      ]
        .join(" ")
        .toLowerCase()
        .includes(keyword)
    })
  }, [searchText, tenantMeta, tenants])

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
      load(selectedNamespace, selectedState)
      loadStateCounts(selectedNamespace)
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Delete failed"))
    } finally {
      setDeleting(null)
    }
  }

  const handleCopyTenantName = async (name: string) => {
    try {
      await navigator.clipboard.writeText(name)
      toast.success(t("Name copied"))
    } catch {
      toast.error(t("Copy failed"))
    }
  }

  const handleOpenConsole = (endpoint: string) => {
    if (!endpoint || endpoint === "-") {
      toast.warning(t("Endpoint is unavailable"))
      return
    }
    window.open(endpoint, "_blank", "noopener,noreferrer")
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

      <div className="mt-4 grid gap-3 sm:grid-cols-2 xl:grid-cols-4 2xl:grid-cols-7">
        <button
          type="button"
          onClick={() => setSelectedState(null)}
          className={`rounded-md border bg-background px-3 py-3 text-left transition ${
            selectedState === null
              ? "border-slate-300 ring-1 ring-slate-200"
              : "border-border hover:border-muted-foreground/40"
          }`}
        >
          <div className="flex items-center justify-between">
            <span className="text-xs font-medium text-slate-700">{t("Total")}</span>
            <span className="size-2 rounded-full bg-slate-500" />
          </div>
          <p className="mt-2 text-3xl leading-none font-semibold">{totalCount}</p>
          <p className="mt-1 text-[11px] text-muted-foreground">
            {selectedState === null ? t("Filtered") : t("Click to filter")}
          </p>
        </button>
        {TENANT_STATES.map((state) => {
          const theme = STATE_THEME[state]
          const active = selectedState === state
          return (
            <button
              key={state}
              type="button"
              onClick={() => setSelectedState((prev) => (prev === state ? null : state))}
              className={`rounded-md border bg-background px-3 py-3 text-left transition ${
                active ? theme.activeCard : "border-border hover:border-muted-foreground/40"
              }`}
            >
              <div className="flex items-center justify-between">
                <span className={`text-xs font-medium ${theme.label}`}>{t(state)}</span>
                <span className={`size-2 rounded-full ${theme.dot}`} />
              </div>
              <p className="mt-2 text-3xl leading-none font-semibold">{stateCounts[state]}</p>
              <p className="mt-1 text-[11px] text-muted-foreground">{active ? t("Filtered") : t("Click to filter")}</p>
            </button>
          )
        })}
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
        {selectedState && (
          <Button type="button" variant="outline" size="sm" onClick={() => setSelectedState(null)}>
            {t("All States")}
          </Button>
        )}
        <Button asChild variant="outline" size="sm">
          <Link href={`${routes.dashboard}#cluster`} prefetch={false}>
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
          {searchText || selectedState
            ? t("No tenants match the current filters.")
            : t("No tenants yet. Create one to get started.")}
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
                <TableHead className="w-[90px]">{t("Actions")}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {filteredTenants.map((tenant) => {
                const key = makeTenantKey(tenant.namespace, tenant.name)
                const meta = tenantMeta[key]
                const normalizedState = normalizeTenantState(tenant.state)
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
                      <span
                        className={`inline-flex rounded border px-2 py-0.5 text-xs ${STATE_THEME[normalizedState].badge}`}
                      >
                        {t(normalizedState)}
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
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button variant="ghost" size="icon-sm" aria-label={t("Actions")} disabled={deleting === key}>
                            {deleting === key ? <Spinner className="size-4" /> : <RiMore2Line className="size-4" />}
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end" className="w-44">
                          <DropdownMenuItem
                            onSelect={() => router.push(routes.tenantDetail(tenant.namespace, tenant.name))}
                          >
                            <RiEyeLine className="size-4" />
                            {t("View Details")}
                          </DropdownMenuItem>
                          <DropdownMenuItem
                            onSelect={() =>
                              router.push(`${routes.tenantDetail(tenant.namespace, tenant.name)}&tab=yaml&editable=1`)
                            }
                          >
                            <RiPencilLine className="size-4" />
                            {t("Edit")}
                          </DropdownMenuItem>
                          <DropdownMenuItem onSelect={() => handleCopyTenantName(tenant.name)}>
                            <RiFileCopyLine className="size-4" />
                            {t("Copy Name")}
                          </DropdownMenuItem>
                          <DropdownMenuItem onSelect={() => handleOpenConsole(meta?.endpoint ?? "-")}>
                            <RiExternalLinkLine className="size-4" />
                            {t("Open Console")}
                          </DropdownMenuItem>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem
                            variant="destructive"
                            onSelect={() => handleDelete(tenant.namespace, tenant.name)}
                            disabled={deleting === key}
                          >
                            <RiDeleteBinLine className="size-4" />
                            {t("Delete")}
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
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
