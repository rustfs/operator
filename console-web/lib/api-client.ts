import { config, getApiBaseUrl } from "@/lib/config"

interface ApiError {
  message: string
  statusCode?: number
}

class ApiClient {
  private getBaseUrl(): string {
    return typeof window !== "undefined" ? getApiBaseUrl() : config.apiBaseUrl
  }

  private async request<T>(endpoint: string, options: RequestInit = {}): Promise<T> {
    const baseUrl = this.getBaseUrl()
    const url = `${baseUrl}${endpoint}`

    const defaultHeaders: Record<string, string> = {
      "Content-Type": "application/json",
    }

    const response = await fetch(url, {
      ...options,
      headers: {
        ...defaultHeaders,
        ...options.headers,
      },
      credentials: "include",
    })

    if (!response.ok) {
      const error: ApiError = {
        message: response.statusText,
        statusCode: response.status,
      }

      try {
        const body = await response.json()
        error.message = body.message || body.error || response.statusText
      } catch {
        // ignore parse errors
      }

      throw error
    }

    return response.json()
  }

  async getText(endpoint: string): Promise<string> {
    const url = `${this.getBaseUrl()}${endpoint}`
    const response = await fetch(url, {
      method: "GET",
      headers: { Accept: "text/plain" },
      credentials: "include",
    })
    if (!response.ok) {
      const text = await response.text()
      throw { message: text || response.statusText, statusCode: response.status }
    }
    return response.text()
  }

  async get<T>(endpoint: string): Promise<T> {
    return this.request<T>(endpoint, { method: "GET" })
  }

  async post<T>(endpoint: string, body?: unknown): Promise<T> {
    return this.request<T>(endpoint, {
      method: "POST",
      body: body ? JSON.stringify(body) : undefined,
    })
  }

  async put<T>(endpoint: string, body?: unknown): Promise<T> {
    return this.request<T>(endpoint, {
      method: "PUT",
      body: body ? JSON.stringify(body) : undefined,
    })
  }

  async delete<T>(endpoint: string): Promise<T> {
    return this.request<T>(endpoint, { method: "DELETE" })
  }
}

export const apiClient = new ApiClient()
export type { ApiError }
