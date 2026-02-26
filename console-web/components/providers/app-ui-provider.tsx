"use client"

import type { ReactNode } from "react"
import { Toaster } from "@/components/ui/sonner"

export function AppUiProvider({ children }: { children: ReactNode }) {
  return (
    <>
      {children}
      <Toaster position="top-center" richColors closeButton />
    </>
  )
}
