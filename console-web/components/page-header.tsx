import { cn } from "@/lib/utils"

export function PageHeader({
  children,
  actions,
  className,
}: {
  children: React.ReactNode
  actions?: React.ReactNode
  className?: string
}) {
  return (
    <div className={cn("sticky top-0 z-10 flex items-center justify-between bg-background py-4", className)}>
      <div>{children}</div>
      <div className="flex items-center gap-2">{actions}</div>
    </div>
  )
}
