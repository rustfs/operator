"use client"

import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { toast } from "sonner"
import { RiAddLine, RiNodeTree, RiServerLine } from "@remixicon/react"
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
import type { ClusterResourcesResponse, NamespaceItem, NodeInfo } from "@/types/api"

type ClusterTab = "nodes" | "resources" | "namespaces"

export default function DashboardPage() {
  const { t } = useTranslation()
  const [tab, setTab] = useState<ClusterTab>("nodes")
  const [nodes, setNodes] = useState<NodeInfo[]>([])
  const [namespaces, setNamespaces] = useState<NamespaceItem[]>([])
  const [resources, setResources] = useState<ClusterResourcesResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [newNsOpen, setNewNsOpen] = useState(false)
  const [newNsName, setNewNsName] = useState("")
  const [createLoading, setCreateLoading] = useState(false)

  const load = async () => {
    setLoading(true)
    try {
      const [nodeRes, nsRes, resRes] = await Promise.all([
        api.listNodes(),
        api.listNamespaces(),
        api.getClusterResources(),
      ])
      setNodes(nodeRes.nodes)
      setNamespaces(nsRes.namespaces)
      setResources(resRes)
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
          <CardContent />
        </Card>
      </div>

      <section id="cluster" className="space-y-4 scroll-mt-6">
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <RiNodeTree className="size-5" />
              <CardTitle className="text-base">{t("Cluster")}</CardTitle>
            </div>
            <CardDescription className="text-sm">
              {t("Cluster nodes, capacity and namespaces.")}
            </CardDescription>
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
                              <TableCell>{node.memory_capacity}</TableCell>
                              <TableCell>{node.cpu_allocatable}</TableCell>
                              <TableCell>{node.memory_allocatable}</TableCell>
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
                        <p className="text-2xl font-semibold">{resources.total_memory}</p>
                      </CardContent>
                    </Card>
                    <Card>
                      <CardHeader>
                        <CardTitle className="text-sm">{t("Allocatable")}</CardTitle>
                      </CardHeader>
                      <CardContent>
                        <p className="text-sm">CPU: {resources.allocatable_cpu}</p>
                        <p className="text-sm">Memory: {resources.allocatable_memory}</p>
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
