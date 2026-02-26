"use client"

import { useTranslation } from "react-i18next"
import { RiServerLine } from "@remixicon/react"
import { Page } from "@/components/page"
import { PageHeader } from "@/components/page-header"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"

export default function DashboardPage() {
  const { t } = useTranslation()

  return (
    <Page>
      <PageHeader>
        <h1 className="text-sm font-semibold">{t("Dashboard")}</h1>
      </PageHeader>

      <Card>
        <CardHeader>
          <div className="flex items-center gap-2">
            <RiServerLine className="size-4" />
            <CardTitle className="text-sm">{t("Welcome to RustFS Operator Console")}</CardTitle>
          </div>
          <CardDescription className="text-xs">
            {t("Manage your RustFS tenants and clusters from this dashboard.")}
          </CardDescription>
        </CardHeader>
        <CardContent>
          <p className="text-xs text-muted-foreground">
            {t("Tenants")} / {t("Dashboard")}
          </p>
        </CardContent>
      </Card>
    </Page>
  )
}
