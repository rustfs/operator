"use client"

import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { toast } from "sonner"
import { RiAddLine, RiArrowDownSLine, RiFolderLine, RiHardDrive2Line, RiNodeTree, RiServerLine } from "@remixicon/react"
import { Page } from "@/components/page"
import { PageHeader } from "@/components/page-header"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Spinner } from "@/components/ui/spinner"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import * as api from "@/lib/api"
import { ApiError } from "@/lib/api-client"
import { cn, formatBinaryBytes, formatK8sMemory } from "@/lib/utils"
import type { ClusterResourcesResponse, NamespaceItem, NodeInfo } from "@/types/api"
import type { TopologyOverviewResponse, TopologyTenantState } from "@/types/topology"

type ClusterTab = "nodes" | "resources" | "namespaces"

const STATE_THEME: Record<
  TopologyTenantState,
  {
    badge: string
    dot: string
    card: string
  }
> = {
  Ready: {
    badge: "border-emerald-200 bg-emerald-50 text-emerald-700",
    dot: "bg-emerald-500",
    card: "border-emerald-200 bg-emerald-50/60",
  },
  Updating: {
    badge: "border-blue-200 bg-blue-50 text-blue-700",
    dot: "bg-blue-500",
    card: "border-blue-200 bg-blue-50/60",
  },
  Degraded: {
    badge: "border-amber-200 bg-amber-50 text-amber-700",
    dot: "bg-amber-500",
    card: "border-amber-200 bg-amber-50/60",
  },
  NotReady: {
    badge: "border-red-200 bg-red-50 text-red-700",
    dot: "bg-red-500",
    card: "border-red-200 bg-red-50/60",
  },
  Unknown: {
    badge: "border-zinc-200 bg-zinc-100 text-zinc-700",
    dot: "bg-zinc-500",
    card: "border-zinc-200 bg-zinc-100/80",
  },
}

function getTreeDotClass(state: string): string {
  switch (state) {
    case "Ready":
    case "Running":
      return "bg-emerald-500"
    case "Updating":
      return "bg-blue-500"
    case "Degraded":
    case "Pending":
      return "bg-amber-500"
    case "NotReady":
    case "Failed":
      return "bg-red-500"
    default:
      return "bg-zinc-400"
  }
}

export default function DashboardPage() {
  const { t } = useTranslation()
  const [tab, setTab] = useState<ClusterTab>("nodes")
  const [nodes, setNodes] = useState<NodeInfo[]>([])
  const [namespaces, setNamespaces] = useState<NamespaceItem[]>([])
  const [resources, setResources] = useState<ClusterResourcesResponse | null>(null)
  const [topology, setTopology] = useState<TopologyOverviewResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [newNsOpen, setNewNsOpen] = useState(false)
  const [newNsName, setNewNsName] = useState("")
  const [createLoading, setCreateLoading] = useState(false)
  const [treeCollapsed, setTreeCollapsed] = useState<Set<string>>(new Set())

  const toggleTreeNode = (id: string) => {
    setTreeCollapsed((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }
  const isTreeExpanded = (id: string) => !treeCollapsed.has(id)

  const load = async () => {
    setLoading(true)
    try {
      const [nodeRes, nsRes, resRes, topologyRes] = await Promise.all([
        api.listNodes(),
        api.listNamespaces(),
        api.getClusterResources(),
        api.getTopologyOverview(),
      ])
      setNodes(nodeRes.nodes)
      setNamespaces(nsRes.namespaces)
      setResources(resRes)
      setTopology(topologyRes)
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Failed to load cluster data"))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    load()
  }, []) // eslint-disable-line react-hooks/exhaustive-deps -- run once on mount

  const handleCreateNamespace = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!newNsName.trim()) {
      toast.warning(t("Namespace name is required"))
      return
    }
    setCreateLoading(true)
    try {
      await api.createNamespace(newNsName.trim())
      toast.success(t("Namespace created"))
      setNewNsOpen(false)
      setNewNsName("")
      load()
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Create failed"))
    } finally {
      setCreateLoading(false)
    }
  }

  const tabs: { id: ClusterTab; labelKey: string }[] = [
    { id: "nodes", labelKey: "Nodes" },
    { id: "resources", labelKey: "Resources" },
    { id: "namespaces", labelKey: "Namespaces" },
  ]

  const topologySummary = topology?.cluster.summary
  const tenantCount = topology?.namespaces.reduce((sum, ns) => sum + ns.tenants.length, 0) ?? 0
  const unhealthyCount =
    topology?.namespaces.reduce((sum, ns) => sum + ns.tenants.filter((t) => t.state !== "Ready").length, 0) ?? 0

  const allPods = topology?.namespaces.flatMap((ns) => ns.tenants.flatMap((t) => t.pods ?? [])) ?? []
  const podTotal = allPods.length
  const podRunning = allPods.filter((p) => p.phase === "Running").length

  const totalCapacityBytes =
    topology?.namespaces.reduce(
      (sum, ns) => sum + ns.tenants.reduce((s, t) => s + (t.summary.capacity_bytes ?? 0), 0),
      0,
    ) ?? 0

  return (
    <Page>
      <PageHeader>
        <h1 className="text-base font-medium">{t("Dashboard")}</h1>
      </PageHeader>

      <div className="grid gap-4">
        <Card className="transition-shadow hover:shadow-md">
          <CardHeader>
            <div className="flex items-center gap-2">
              <RiServerLine className="size-5" />
              <CardTitle className="text-base">{t("Tenants")}</CardTitle>
            </div>
            <CardDescription className="text-sm">
              {t("Manage RustFS tenants: create, view, edit pools and pods.")}
            </CardDescription>
          </CardHeader>
          <CardContent className="grid gap-3 sm:grid-cols-3">
            <div className="rounded-md border border-border bg-background px-3 py-3">
              <p className="text-[11px] uppercase tracking-[0.2em] text-muted-foreground">{t("Tenants")}</p>
              <div className="mt-2 flex items-baseline gap-2">
                <span className="text-2xl font-semibold">{topologySummary?.tenants ?? tenantCount}</span>
                {unhealthyCount > 0 && (
                  <span className="text-sm font-medium text-destructive">
                    {unhealthyCount} {t("unhealthy")}
                  </span>
                )}
              </div>
            </div>
            <div className="rounded-md border border-border bg-background px-3 py-3">
              <p className="text-[11px] uppercase tracking-[0.2em] text-muted-foreground">{t("Pods")}</p>
              <div className="mt-2 flex items-baseline gap-2">
                <span className="text-2xl font-semibold">{podTotal}</span>
                <span className="text-sm font-medium text-emerald-600 dark:text-emerald-400">
                  {podRunning} {t("running")}
                </span>
              </div>
            </div>
            <div className="rounded-md border border-border bg-background px-3 py-3">
              <p className="text-[11px] uppercase tracking-[0.2em] text-muted-foreground">{t("Capacity")}</p>
              <p className="mt-2 text-2xl font-semibold">{formatBinaryBytes(totalCapacityBytes)}</p>
            </div>
          </CardContent>
        </Card>
      </div>

      {topology && topology.namespaces.length > 0 && (
        <Card className="vtree-card">
          <CardHeader className="border-b">
            <div className="flex items-center gap-2">
              <RiNodeTree className="size-5 text-muted-foreground" />
              <CardTitle className="text-base">{t("Cluster Topology")}</CardTitle>
            </div>
          </CardHeader>
          <CardContent className="pt-5 pb-6 overflow-x-auto">
            <div className="vtree-v">
              {/* Cluster root */}
              <div className="vtree-vnode">
                <button
                  type="button"
                  className="vtree-vbox vtree-vbox--cluster"
                  onClick={() => toggleTreeNode("cluster")}
                >
                  <RiServerLine className="size-4" />
                  <span className="vtree-vbox-name">{topology.cluster.name || t("Cluster")}</span>
                  <span className="vtree-vbadge">{topology.namespaces.length}</span>
                  <RiArrowDownSLine className={cn("vtree-vchevron", !isTreeExpanded("cluster") && "rotate-180")} />
                </button>

                {isTreeExpanded("cluster") && (
                  <div className="vtree-vchildren">
                    {topology.namespaces.map((ns) => {
                      const nsId = `ns:${ns.name}`
                      const nsHasIssues = ns.unhealthy_tenant_count > 0
                      return (
                        <div key={ns.name} className="vtree-vnode">
                          <button
                            type="button"
                            className={cn("vtree-vbox vtree-vbox--ns", nsHasIssues && "vtree-vbox--alert")}
                            onClick={() => toggleTreeNode(nsId)}
                          >
                            <RiFolderLine className="size-4" />
                            <span className="vtree-vbox-name">{ns.name}</span>
                            <span className="vtree-vbadge">{ns.tenants.length}</span>
                            {nsHasIssues && <span className="vtree-valert-dot" />}
                            <RiArrowDownSLine className={cn("vtree-vchevron", !isTreeExpanded(nsId) && "rotate-180")} />
                          </button>

                          {isTreeExpanded(nsId) && (
                            <div className="vtree-vchildren">
                              {ns.tenants.map((tenant) => {
                                const tenantId = `t:${ns.name}/${tenant.name}`
                                const pools = tenant.pools ?? []
                                const pods = tenant.pods ?? []
                                return (
                                  <div key={tenant.name} className="vtree-vnode">
                                    <button
                                      type="button"
                                      className="vtree-vbox vtree-vbox--tenant"
                                      onClick={() => toggleTreeNode(tenantId)}
                                    >
                                      <span className={cn("vtree-vdot", getTreeDotClass(tenant.state))} />
                                      <span className="vtree-vbox-name">{tenant.name}</span>
                                      <span className={cn("vtree-vstate", STATE_THEME[tenant.state].badge)}>
                                        {t(tenant.state)}
                                      </span>
                                      <RiArrowDownSLine
                                        className={cn("vtree-vchevron", !isTreeExpanded(tenantId) && "rotate-180")}
                                      />
                                    </button>

                                    {isTreeExpanded(tenantId) && pools.length > 0 && (
                                      <div className="vtree-vchildren">
                                        {pools.map((pool) => {
                                          const poolId = `p:${ns.name}/${tenant.name}/${pool.name}`
                                          const poolPods = pods.filter((p) => p.pool === pool.name)
                                          return (
                                            <div key={pool.name} className="vtree-vnode">
                                              <button
                                                type="button"
                                                className="vtree-vbox vtree-vbox--pool"
                                                onClick={() => toggleTreeNode(poolId)}
                                              >
                                                <RiHardDrive2Line className="size-3.5" />
                                                <span className="vtree-vbox-name">{pool.name}</span>
                                                <span className="vtree-vmeta">
                                                  {pool.servers}s&middot;{pool.volumes_per_server}v
                                                </span>
                                                <RiArrowDownSLine
                                                  className={cn(
                                                    "vtree-vchevron",
                                                    !isTreeExpanded(poolId) && "rotate-180",
                                                  )}
                                                />
                                              </button>

                                              {isTreeExpanded(poolId) && poolPods.length > 0 && (
                                                <div className="vtree-vpods">
                                                  {poolPods.map((pod) => (
                                                    <div
                                                      key={pod.name}
                                                      className={cn(
                                                        "vtree-vtile",
                                                        pod.phase === "Running"
                                                          ? "vtree-vtile--ok"
                                                          : pod.phase === "Pending"
                                                            ? "vtree-vtile--warn"
                                                            : "vtree-vtile--error",
                                                      )}
                                                    >
                                                      <div className="vtree-vtile-tip">
                                                        <p className="vtree-vtile-tip-name">{pod.name}</p>
                                                        <div className="vtree-vtile-tip-rows">
                                                          <span className="vtree-vtile-tip-label">{t("Phase")}</span>
                                                          <span
                                                            className={cn(
                                                              "vtree-vtile-tip-val",
                                                              pod.phase === "Running"
                                                                ? "text-emerald-600 dark:text-emerald-400"
                                                                : pod.phase === "Pending"
                                                                  ? "text-amber-600 dark:text-amber-400"
                                                                  : "text-red-600 dark:text-red-400",
                                                            )}
                                                          >
                                                            {pod.phase}
                                                          </span>
                                                          <span className="vtree-vtile-tip-label">{t("Node")}</span>
                                                          <span className="vtree-vtile-tip-val">{pod.node ?? "-"}</span>
                                                          <span className="vtree-vtile-tip-label">{t("Ready")}</span>
                                                          <span className="vtree-vtile-tip-val">{pod.ready}</span>
                                                        </div>
                                                      </div>
                                                    </div>
                                                  ))}
                                                </div>
                                              )}
                                            </div>
                                          )
                                        })}
                                      </div>
                                    )}
                                  </div>
                                )
                              })}
                            </div>
                          )}
                        </div>
                      )
                    })}
                  </div>
                )}
              </div>
            </div>
          </CardContent>
        </Card>
      )}

      <section id="cluster" className="space-y-4 scroll-mt-6">
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <RiNodeTree className="size-5" />
              <CardTitle className="text-base">{t("Cluster")}</CardTitle>
            </div>
            <CardDescription className="text-sm">{t("Cluster nodes, capacity and namespaces.")}</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="flex gap-2 border-b border-border">
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

            {loading ? (
              <div className="flex items-center justify-center py-12">
                <Spinner className="size-8" />
              </div>
            ) : (
              <>
                {tab === "nodes" && (
                  <div className="rounded-md border border-border">
                    <Table>
                      <TableHeader>
                        <TableRow>
                          <TableHead>{t("Name")}</TableHead>
                          <TableHead>{t("Status")}</TableHead>
                          <TableHead>{t("Roles")}</TableHead>
                          <TableHead>{t("CPU Capacity")}</TableHead>
                          <TableHead>{t("Memory Capacity")}</TableHead>
                          <TableHead>{t("CPU Allocatable")}</TableHead>
                          <TableHead>{t("Memory Allocatable")}</TableHead>
                        </TableRow>
                      </TableHeader>
                      <TableBody>
                        {nodes.length === 0 ? (
                          <TableRow>
                            <TableCell colSpan={7} className="py-8 text-center text-muted-foreground">
                              {t("No nodes")}
                            </TableCell>
                          </TableRow>
                        ) : (
                          nodes.map((node) => (
                            <TableRow key={node.name}>
                              <TableCell className="font-medium">{node.name}</TableCell>
                              <TableCell>{node.status}</TableCell>
                              <TableCell>{node.roles.join(", ") || "-"}</TableCell>
                              <TableCell>{node.cpu_capacity}</TableCell>
                              <TableCell>{formatK8sMemory(node.memory_capacity)}</TableCell>
                              <TableCell>{node.cpu_allocatable}</TableCell>
                              <TableCell>{formatK8sMemory(node.memory_allocatable)}</TableCell>
                            </TableRow>
                          ))
                        )}
                      </TableBody>
                    </Table>
                  </div>
                )}

                {tab === "resources" && resources && (
                  <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
                    <Card>
                      <CardHeader>
                        <CardTitle className="text-sm">{t("Total Nodes")}</CardTitle>
                      </CardHeader>
                      <CardContent>
                        <p className="text-2xl font-semibold">{resources.total_nodes}</p>
                      </CardContent>
                    </Card>
                    <Card>
                      <CardHeader>
                        <CardTitle className="text-sm">{t("Total CPU")}</CardTitle>
                      </CardHeader>
                      <CardContent>
                        <p className="text-2xl font-semibold">{resources.total_cpu}</p>
                      </CardContent>
                    </Card>
                    <Card>
                      <CardHeader>
                        <CardTitle className="text-sm">{t("Total Memory")}</CardTitle>
                      </CardHeader>
                      <CardContent>
                        <p className="text-2xl font-semibold">{formatK8sMemory(resources.total_memory)}</p>
                      </CardContent>
                    </Card>
                    <Card>
                      <CardHeader>
                        <CardTitle className="text-sm">{t("Allocatable")}</CardTitle>
                      </CardHeader>
                      <CardContent>
                        <p className="text-sm">CPU: {resources.allocatable_cpu}</p>
                        <p className="text-sm">Memory: {formatK8sMemory(resources.allocatable_memory)}</p>
                      </CardContent>
                    </Card>
                  </div>
                )}

                {tab === "namespaces" && (
                  <div className="space-y-4">
                    <div className="flex justify-end">
                      <Button size="sm" onClick={() => setNewNsOpen(true)}>
                        <RiAddLine className="mr-1 size-4" />
                        {t("Create Namespace")}
                      </Button>
                    </div>
                    {newNsOpen && (
                      <Card>
                        <CardHeader>
                          <CardTitle className="text-base">{t("Create Namespace")}</CardTitle>
                          <CardDescription>{t("Create a new Kubernetes namespace.")}</CardDescription>
                        </CardHeader>
                        <CardContent>
                          <form onSubmit={handleCreateNamespace} className="flex items-end gap-4">
                            <div className="max-w-xs flex-1 space-y-2">
                              <Label htmlFor="ns-name">{t("Name")}</Label>
                              <Input
                                id="ns-name"
                                value={newNsName}
                                onChange={(e) => setNewNsName(e.target.value)}
                                placeholder="my-namespace"
                              />
                            </div>
                            <Button type="submit" disabled={createLoading}>
                              {createLoading && <Spinner className="mr-2 size-4" />}
                              {t("Create")}
                            </Button>
                            <Button
                              type="button"
                              variant="outline"
                              onClick={() => {
                                setNewNsOpen(false)
                                setNewNsName("")
                              }}
                            >
                              {t("Cancel")}
                            </Button>
                          </form>
                        </CardContent>
                      </Card>
                    )}
                    <div className="rounded-md border border-border">
                      <Table>
                        <TableHeader>
                          <TableRow>
                            <TableHead>{t("Name")}</TableHead>
                            <TableHead>{t("Status")}</TableHead>
                            <TableHead>{t("Created")}</TableHead>
                          </TableRow>
                        </TableHeader>
                        <TableBody>
                          {namespaces.map((ns) => (
                            <TableRow key={ns.name}>
                              <TableCell className="font-medium">{ns.name}</TableCell>
                              <TableCell>{ns.status}</TableCell>
                              <TableCell className="text-muted-foreground">
                                {ns.created_at ? new Date(ns.created_at).toLocaleString() : "-"}
                              </TableCell>
                            </TableRow>
                          ))}
                        </TableBody>
                      </Table>
                    </div>
                  </div>
                )}
              </>
            )}
          </CardContent>
        </Card>
      </section>
    </Page>
  )
}
