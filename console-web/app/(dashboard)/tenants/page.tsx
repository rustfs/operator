"use client"

import { useEffect, useState } from "react"
import Link from "next/link"
import { useTranslation } from "react-i18next"
import { toast } from "sonner"
import { RiAddLine, RiEyeLine, RiDeleteBinLine } from "@remixicon/react"
import { Page } from "@/components/page"
import { PageHeader } from "@/components/page-header"
import { Button } from "@/components/ui/button"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import { Spinner } from "@/components/ui/spinner"
import { routes } from "@/lib/routes"
import * as api from "@/lib/api"
import type { TenantListItem } from "@/types/api"
import { ApiError } from "@/lib/api-client"

export default function TenantsListPage() {
  const { t } = useTranslation()
  const [tenants, setTenants] = useState<TenantListItem[]>([])
  const [loading, setLoading] = useState(true)
  const [deleting, setDeleting] = useState<string | null>(null)

  const load = async () => {
    setLoading(true)
    try {
      const res = await api.listTenants()
      setTenants(res.tenants)
    } catch (e) {
      const err = e as ApiError
      toast.error(err.message || t("Failed to load tenants"))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    load()
  }, []) // eslint-disable-line react-hooks/exhaustive-deps -- run once on mount

  const handleDelete = async (namespace: string, name: string) => {
    if (!confirm(t("Delete tenant \"{{name}}\"? This cannot be undone.", { name }))) return
    setDeleting(`${namespace}/${name}`)
    try {
      await api.deleteTenant(namespace, name)
      toast.success(t("Tenant deleted"))
      load()
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
      </PageHeader>

      {loading ? (
        <div className="flex items-center justify-center py-12">
          <Spinner className="size-8" />
        </div>
      ) : tenants.length === 0 ? (
        <div className="rounded-lg border border-dashed border-border py-12 text-center text-sm text-muted-foreground">
          {t("No tenants yet. Create one to get started.")}
          <div className="mt-4">
            <Button asChild size="sm">
              <Link href={routes.tenantNew} prefetch={false}>{t("Create Tenant")}</Link>
            </Button>
          </div>
        </div>
      ) : (
        <div className="rounded-md border border-border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t("Namespace")}</TableHead>
                <TableHead>{t("Name")}</TableHead>
                <TableHead>{t("State")}</TableHead>
                <TableHead>{t("Pools")}</TableHead>
                <TableHead>{t("Created")}</TableHead>
                <TableHead className="w-[120px]">{t("Actions")}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {tenants.map((tnt) => (
                <TableRow key={`${tnt.namespace}/${tnt.name}`}>
                  <TableCell className="font-medium">{tnt.namespace}</TableCell>
                  <TableCell>{tnt.name}</TableCell>
                  <TableCell>{tnt.state}</TableCell>
                  <TableCell>
                    {tnt.pools.length === 0
                      ? "-"
                      : tnt.pools.map((p) => p.name).join(", ")}
                  </TableCell>
                  <TableCell className="text-muted-foreground">
                    {tnt.created_at
                      ? new Date(tnt.created_at).toLocaleString()
                      : "-"}
                  </TableCell>
                  <TableCell>
                    <div className="flex gap-1">
                      <Button asChild variant="ghost" size="icon-sm">
                        <Link
                          href={routes.tenantDetail(tnt.namespace, tnt.name)}
                          prefetch={false}
                          title={t("View")}
                        >
                          <RiEyeLine className="size-4" />
                        </Link>
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        className="text-destructive hover:text-destructive"
                        disabled={deleting === `${tnt.namespace}/${tnt.name}`}
                        onClick={() => handleDelete(tnt.namespace, tnt.name)}
                        title={t("Delete")}
                      >
                        {deleting === `${tnt.namespace}/${tnt.name}` ? (
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
      )}
    </Page>
  )
}
