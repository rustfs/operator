"use client"

import { createContext, useContext, useCallback, useEffect, useMemo, useState, type ReactNode } from "react"
import { useRouter } from "next/navigation"
import { apiClient } from "@/lib/api-client"
import { routes } from "@/lib/routes"
import type { LoginResponse, SessionResponse } from "@/types/auth"

interface AuthContextType {
  isAuthenticated: boolean
  isLoading: boolean
  login: (token: string) => Promise<LoginResponse>
  logout: () => Promise<void>
  checkSession: () => Promise<boolean>
}

const AuthContext = createContext<AuthContextType | null>(null)

export function AuthProvider({ children }: { children: ReactNode }) {
  const router = useRouter()
  const [isAuthenticated, setIsAuthenticated] = useState(false)
  const [isLoading, setIsLoading] = useState(true)

  const checkSession = useCallback(async (): Promise<boolean> => {
    try {
      const res = await apiClient.get<SessionResponse>("/session")
      setIsAuthenticated(res.valid)
      return res.valid
    } catch {
      setIsAuthenticated(false)
      return false
    }
  }, [])

  const login = useCallback(
    async (token: string): Promise<LoginResponse> => {
      const res = await apiClient.post<LoginResponse>("/login", { token })
      if (res.success) {
        setIsAuthenticated(true)
        router.push(routes.dashboard)
      }
      return res
    },
    [router],
  )

  const logout = useCallback(async () => {
    try {
      await apiClient.post("/logout")
    } catch {
      // ignore errors on logout
    }
    setIsAuthenticated(false)
    router.push(routes.login)
  }, [router])

  useEffect(() => {
    // Sync with external auth API on mount; setState runs in async .finally() callback
    // eslint-disable-next-line react-hooks/set-state-in-effect
    checkSession().finally(() => setIsLoading(false))
  }, [checkSession])

  const value = useMemo(
    () => ({
      isAuthenticated,
      isLoading,
      login,
      logout,
      checkSession,
    }),
    [isAuthenticated, isLoading, login, logout, checkSession],
  )

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>
}

export function useAuth() {
  const context = useContext(AuthContext)
  if (!context) {
    throw new Error("useAuth must be used within AuthProvider")
  }
  return context
}
