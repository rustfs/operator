"use client"

import { useTranslation } from "react-i18next"
import Link from "next/link"
import { RiServerLine, RiNodeTree } from "@remixicon/react"
import { Page } from "@/components/page"
import { PageHeader } from "@/components/page-header"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Button } from "@/components/ui/button"
import { routes } from "@/lib/routes"

export default function DashboardPage() {
  const { t } = useTranslation()

  return (
    <Page>
      <PageHeader>
        <h1 className="text-base font-medium">{t("Dashboard")}</h1>
      </PageHeader>

      <div className="grid gap-4 sm:grid-cols-2">
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
          <CardContent>
            <Button asChild variant="default" size="sm">
              <Link href={routes.tenants} prefetch={false}>{t("View Tenants")}</Link>
            </Button>
          </CardContent>
        </Card>

        <Card className="transition-shadow hover:shadow-md">
          <CardHeader>
            <div className="flex items-center gap-2">
              <RiNodeTree className="size-5" />
              <CardTitle className="text-base">{t("Cluster")}</CardTitle>
            </div>
            <CardDescription className="text-sm">
              {t("View cluster nodes, resources and namespaces.")}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Button asChild variant="default" size="sm">
              <Link href={routes.cluster} prefetch={false}>{t("View Cluster")}</Link>
            </Button>
          </CardContent>
        </Card>
      </div>
    </Page>
  )
}
