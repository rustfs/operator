"use client"

import { usePathname } from "next/navigation"
import Link from "next/link"
import { useTranslation } from "react-i18next"
import {
  RiDashboardLine,
  RiServerLine,
  RiLogoutBoxLine,
  RiNodeTree,
} from "@remixicon/react"
import { AuthGuard } from "@/components/auth-guard"
import { Button } from "@/components/ui/button"
import { Separator } from "@/components/ui/separator"
import { useAuth } from "@/contexts/auth-context"
import { routes } from "@/lib/routes"
import { cn } from "@/lib/utils"

const navItems = [
  { href: routes.dashboard, icon: RiDashboardLine, labelKey: "Dashboard" },
  { href: routes.tenants, icon: RiServerLine, labelKey: "Tenants" },
  { href: routes.cluster, icon: RiNodeTree, labelKey: "Cluster" },
]

export default function DashboardLayout({
  children,
}: {
  children: React.ReactNode
}) {
  const { t } = useTranslation()
  const { logout } = useAuth()
  const pathname = usePathname()

  return (
    <AuthGuard>
      <div className="flex min-h-screen flex-col">
        <header className="flex h-12 items-center justify-between border-b border-border px-4">
          <div className="flex items-center gap-4">
            <RiDashboardLine className="size-5 text-muted-foreground" />
            <span className="text-sm font-semibold">
              {t("RustFS Operator Console")}
            </span>
          </div>
          <Button variant="ghost" size="sm" className="h-8 text-xs" onClick={logout}>
            <RiLogoutBoxLine className="mr-1 size-3.5" />
            {t("Logout")}
          </Button>
        </header>
        <div className="flex flex-1">
          <aside className="w-52 shrink-0 border-r border-border bg-muted/30 py-4">
            <nav className="flex flex-col gap-0.5 px-2">
              {navItems.map((item) => {
                const Icon = item.icon
                const isActive =
                  pathname === item.href ||
                  (item.href !== routes.dashboard &&
                    pathname.startsWith(item.href))
                return (
                  <Link
                    key={item.href}
                    href={item.href}
                    prefetch={false}
                    className={cn(
                      "flex items-center gap-2 rounded-md px-3 py-2 text-sm font-medium transition-colors",
                      isActive
                        ? "bg-primary text-primary-foreground"
                        : "text-muted-foreground hover:bg-muted hover:text-foreground"
                    )}
                  >
                    <Icon className="size-4 shrink-0" />
                    {t(item.labelKey)}
                  </Link>
                )
              })}
            </nav>
          </aside>
          <main className="flex-1 overflow-auto p-6">{children}</main>
        </div>
      </div>
    </AuthGuard>
  )
}
