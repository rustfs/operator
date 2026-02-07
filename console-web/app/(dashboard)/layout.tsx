"use client"

import { useTranslation } from "react-i18next"
import { RiDashboardLine, RiLogoutBoxLine } from "@remixicon/react"
import { AuthGuard } from "@/components/auth-guard"
import { Button } from "@/components/ui/button"
import { Separator } from "@/components/ui/separator"
import { useAuth } from "@/contexts/auth-context"

export default function DashboardLayout({
  children,
}: {
  children: React.ReactNode
}) {
  const { t } = useTranslation()
  const { logout } = useAuth()

  return (
    <AuthGuard>
      <div className="flex min-h-screen flex-col">
        {/* Top Navigation Bar */}
        <header className="flex h-10 items-center justify-between border-b border-border px-4">
          <div className="flex items-center gap-3">
            <RiDashboardLine className="size-4" />
            <span className="text-xs font-semibold">{t("RustFS Operator Console")}</span>
          </div>
          <Button variant="ghost" size="sm" className="h-7 text-xs" onClick={logout}>
            <RiLogoutBoxLine className="mr-1 size-3.5" />
            {t("Logout")}
          </Button>
        </header>
        <Separator />
        {/* Main Content */}
        <main className="flex flex-1 flex-col gap-4 p-6 pt-0">{children}</main>
      </div>
    </AuthGuard>
  )
}
