import type { TenantLifecycleState } from "@/types/api"
import type { TopologyTenantState } from "@/types/topology"

function normalizeStateToken(state: string | null | undefined): string {
  return (state ?? "")
    .trim()
    .toLowerCase()
    .replace(/[\s_-]/g, "")
}

export function normalizeTenantLifecycleState(state: string | null | undefined): TenantLifecycleState {
  const normalized = normalizeStateToken(state)

  if (normalized === "ready" || normalized === "running") return "Ready"
  if (
    normalized === "reconciling" ||
    normalized === "updating" ||
    normalized === "creating" ||
    normalized.includes("provision")
  ) {
    return "Reconciling"
  }
  if (normalized === "blocked") return "Blocked"
  if (normalized === "degraded") return "Degraded"
  if (normalized === "notready" || normalized === "error" || normalized.includes("fail")) {
    return "NotReady"
  }
  if (normalized === "unknown" || normalized === "stopped") return "Unknown"
  return "Unknown"
}

export function normalizeTopologyTenantState(state: string | null | undefined): TopologyTenantState {
  return normalizeTenantLifecycleState(state)
}
