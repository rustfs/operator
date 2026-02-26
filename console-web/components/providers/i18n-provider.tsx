"use client"

import { useEffect, useState, type ReactNode } from "react"
import "@/lib/i18n"

export function I18nProvider({ children }: { children: ReactNode }) {
  const [ready, setReady] = useState(false)

  useEffect(() => {
    setReady(true)
  }, [])

  if (!ready) return null

  return <>{children}</>
}
