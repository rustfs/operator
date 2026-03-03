"use client"

import { usePathname } from "next/navigation"
import Link from "next/link"
import Image from "next/image"
import { useTranslation } from "react-i18next"
import {
  RiDashboardLine,
  RiServerLine,
  RiLogoutBoxLine,
  RiNodeTree,
  RiQuestionLine,
  RiGithubLine,
  RiTwitterXLine,
  RiUser3Line,
} from "@remixicon/react"

import { AuthGuard } from "@/components/auth-guard"
import { Button } from "@/components/ui/button"
import { useAuth } from "@/contexts/auth-context"
import { routes } from "@/lib/routes"
import { cn } from "@/lib/utils"
import { LanguageSwitcher } from "@/components/language-switcher"
import { ThemeSwitcher } from "@/components/theme-switcher"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"

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
  const GITHUB_URL = "https://github.com/rustfs/operator"
  const X_URL = "https://x.com/rustfsofficial"

  return (
    <AuthGuard>
      <div className="flex min-h-screen">
        <aside className="w-64 shrink-0 border-r border-border bg-muted/20">
          <div className="flex min-w-0 items-baseline gap-2 px-4 py-6">
            <Link href="/" prefetch={false} className="inline-flex items-center gap-2">
              <Image src="/logo.svg" width={64} height={16} alt="RustFS" className="h-4 w-auto shrink-0" />
            </Link>
          </div>
          <nav className="flex flex-col gap-0.5 px-2">
            {navItems.map((item) => {
              const Icon = item.icon
              const isActive =
                pathname === item.href ||
                (item.href !== routes.dashboard && pathname.startsWith(item.href))
              return (
                <Link
                  key={item.href}
                  href={item.href}
                  prefetch={false}
                  className={cn(
                    "flex items-center gap-3 rounded-none px-2.5 py-2 text-xs font-medium transition-colors",
                    isActive ? "bg-muted text-foreground" : "text-foreground/70 hover:bg-muted"
                  )}
                >
                  <Icon className="size-4 shrink-0" />
                  {t(item.labelKey)}
                </Link>
              )
            })}
          </nav>
        </aside>
        <div className="flex flex-1 flex-col">
          <header className="flex h-16 shrink-0 items-center justify-between gap-2 border-b border-border bg-background px-4">
            <div className="flex items-center gap-3">
              {(() => {
                const activeItem =
                  navItems.find(
                    (item) =>
                      pathname === item.href ||
                      (item.href !== routes.dashboard && pathname.startsWith(item.href)),
                  ) ?? navItems[0]
                const ActiveIcon = activeItem.icon
                return (
                  <>
                    <ActiveIcon className="size-5 text-muted-foreground" />
                    <span className="text-xs font-medium">{t(activeItem.labelKey)}</span>
                  </>
                )
              })()}
            </div>
            <div className="flex items-center gap-1">
              <LanguageSwitcher />
              <ThemeSwitcher />
              {/* <Button variant="ghost" size="icon-sm" aria-label="Help">
                <RiQuestionLine className="size-4" />
              </Button> */}
              {GITHUB_URL && (
                <Button asChild variant="ghost" size="icon-sm" aria-label="GitHub">
                  <Link href={GITHUB_URL} prefetch={false} target="_blank" rel="noopener noreferrer">
                    <RiGithubLine className="size-4" />
                  </Link>
                </Button>
              )}
              {X_URL && (
                <Button asChild variant="ghost" size="icon-sm" aria-label="X">
                  <Link href={X_URL} prefetch={false} target="_blank" rel="noopener noreferrer">
                    <RiTwitterXLine className="size-4" />
                    
                  </Link>
                </Button>
              )}
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant="outline" size="icon-sm" aria-label="User">
                    <RiUser3Line className="size-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-40">
                  <DropdownMenuItem onSelect={logout}>
                    <RiLogoutBoxLine className="me-2 size-4" />
                    {t("Logout")}
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </header>
          <main className="flex-1 overflow-auto p-6 pt-4">{children}</main>
        </div>
      </div>
    </AuthGuard>
  )
}
