import { cn } from "@/lib/utils"

export function PageHeader({
  children,
  description,
  actions,
  className,
  sticky = true,
}: {
  children: React.ReactNode
  description?: React.ReactNode
  actions?: React.ReactNode
  className?: string
  sticky?: boolean
}) {
  return (
    <div className={cn("bg-background flex flex-col justify-between gap-2 lg:flex-row", className)}>
      <div className="space-y-1">
        {children}
        {description}
      </div>
      {actions && <div className="flex flex-1 flex-wrap items-center justify-end gap-2">{actions}</div>}
    </div>
  )
}
