import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

/**
 * Parse a Kubernetes-style size string (e.g. "32883524Ki", "10Gi", "512Mi") to bytes.
 * Returns null if the input is empty or unparseable.
 */
export function parseSizeToBytes(size: string | null | undefined): number | null {
  if (!size) return null
  const match = size.trim().match(/^(\d+(?:\.\d+)?)\s*([kmgtpe]?i?b?)?$/i)
  if (!match) return null

  const value = Number.parseFloat(match[1] ?? "0")
  if (!Number.isFinite(value) || value < 0) return null

  const rawUnit = (match[2] ?? "").toUpperCase().replace(/B$/, "")
  if (!rawUnit) return value

  const binary = rawUnit.endsWith("I")
  const unit = binary ? rawUnit.slice(0, -1) : rawUnit
  const powers: Record<string, number> = { "": 0, K: 1, M: 2, G: 3, T: 4, P: 5, E: 6 }
  const power = powers[unit]
  if (power == null) return null
  const base = binary ? 1024 : 1000
  return value * base ** power
}

/**
 * Format a byte count into a human-readable binary string (TiB, GiB, MiB, B).
 */
export function formatBinaryBytes(bytes: number): string {
  const format = (value: number) => {
    if (Number.isInteger(value)) return String(value)
    return value.toFixed(1).replace(/\.0$/, "")
  }

  if (bytes >= 1024 ** 4) return `${format(bytes / 1024 ** 4)} TiB`
  if (bytes >= 1024 ** 3) return `${format(bytes / 1024 ** 3)} GiB`
  if (bytes >= 1024 ** 2) return `${format(bytes / 1024 ** 2)} MiB`
  return `${format(bytes)} B`
}

/**
 * Parse a K8s size string and format it as a human-readable binary string.
 * Returns the original string if parsing fails.
 */
export function formatK8sMemory(size: string): string {
  const bytes = parseSizeToBytes(size)
  if (bytes == null) return size
  return formatBinaryBytes(bytes)
}
