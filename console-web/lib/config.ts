const basePath = process.env.NEXT_PUBLIC_BASE_PATH ?? ""

// Build-time default: relative /api/v1 = same-origin (Ingress: / -> frontend, /api -> backend)
const rawApi = process.env.NEXT_PUBLIC_API_BASE_URL ?? ""
const buildTimeApiBaseUrl = rawApi || "/api/v1"

export const config = {
  basePath,
  /** Build-time default; use getApiBaseUrl() in browser for runtime override. */
  apiBaseUrl: buildTimeApiBaseUrl,
}

const STORAGE_KEY = "rustfs_console_api_base_url"

/**
 * Effective API base URL: query param (saved to localStorage) > localStorage > build-time.
 * Use this in browser so ?apiBaseUrl= works without rebuild. localStorage (not sessionStorage)
 * so the same origin in new tabs also gets the same API base URL after login.
 */
export function getApiBaseUrl(): string {
  if (typeof window === "undefined") return buildTimeApiBaseUrl
  const params = new URLSearchParams(window.location.search)
  const fromQuery = params.get("apiBaseUrl")
  if (fromQuery) {
    try {
      localStorage.setItem(STORAGE_KEY, fromQuery)
    } catch {
      /* ignore */
    }
    return fromQuery
  }
  try {
    const fromStorage = localStorage.getItem(STORAGE_KEY)
    if (fromStorage) return fromStorage
  } catch {
    /* ignore */
  }
  return buildTimeApiBaseUrl
}
