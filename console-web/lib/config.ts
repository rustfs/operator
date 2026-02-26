const basePath = process.env.NEXT_PUBLIC_BASE_PATH ?? ""

// Relative /api/v1 = same-origin (K8s Ingress: / -> frontend, /api -> backend)
const rawApi = process.env.NEXT_PUBLIC_API_BASE_URL ?? ""
export const config = {
  basePath,
  apiBaseUrl: rawApi || "/api/v1",
}
