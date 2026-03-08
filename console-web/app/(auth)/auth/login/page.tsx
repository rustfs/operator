"use client"

import { useState } from "react"
import { useTranslation } from "react-i18next"
import { toast } from "sonner"
import { RiKeyLine, RiShieldKeyholeLine } from "@remixicon/react"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Separator } from "@/components/ui/separator"
import { Spinner } from "@/components/ui/spinner"
import { useAuth } from "@/contexts/auth-context"

export default function LoginPage() {
  const { t } = useTranslation()
  const { login } = useAuth()
  const [token, setToken] = useState("")
  const [loading, setLoading] = useState(false)
  const [showHelp, setShowHelp] = useState(false)

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault()

    if (!token.trim()) {
      toast.warning(t("Token is required"))
      return
    }

    setLoading(true)
    try {
      await login(token.trim())
      toast.success(t("Login successful"))
    } catch (error: unknown) {
      const message =
        error && typeof error === "object" && "message" in error
          ? (error as { message: string }).message
          : t("Login failed")
      toast.error(message)
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="flex w-full max-w-md flex-col gap-6 px-4">
      {/* Logo & Title */}
      <div className="flex flex-col items-center gap-3">
        <div className="flex size-12 items-center justify-center border border-border bg-card">
          <RiShieldKeyholeLine className="size-6 text-foreground" />
        </div>
        <div className="text-center">
          <h1 className="text-sm font-semibold tracking-tight">{t("RustFS Operator Console")}</h1>
        </div>
      </div>

      {/* Login Card */}
      <Card>
        <CardHeader className="px-6 pb-4 pt-6">
          <CardTitle className="text-sm">{t("Login")}</CardTitle>
          <CardDescription className="text-xs">{t("Enter your Kubernetes ServiceAccount token")}</CardDescription>
        </CardHeader>
        <CardContent className="px-6 pb-6">
          <form onSubmit={handleSubmit} className="flex flex-col gap-4">
            <div className="flex flex-col gap-2">
              <Label htmlFor="token" className="text-xs">
                <RiKeyLine className="mr-1 inline-block size-3.5" />
                {t("JWT Token")}
              </Label>
              <Input
                id="token"
                type="password"
                value={token}
                onChange={(e) => setToken(e.target.value)}
                placeholder="eyJhbGciOiJSUzI1NiIs..."
                className="h-8 font-mono text-xs"
                autoComplete="off"
                autoFocus
              />
            </div>

            <Button type="submit" className="h-8 w-full text-xs" disabled={loading}>
              {loading && <Spinner className="mr-2 size-3.5" />}
              {loading ? t("Signing In...") : t("Sign In")}
            </Button>
          </form>

          <Separator className="my-4" />

          {/* Token Help */}
          <div className="flex flex-col gap-2">
            <button
              type="button"
              onClick={() => setShowHelp(!showHelp)}
              className="text-left text-xs text-muted-foreground hover:text-foreground"
            >
              {t("How to get a token")} {showHelp ? "▾" : "▸"}
            </button>

            {showHelp && (
              <div className="flex flex-col gap-2 border border-border bg-muted p-3">
                <p className="text-xs text-muted-foreground">{t("Run the following command to generate a token:")}</p>
                <pre className="overflow-x-auto bg-background p-2 font-mono text-xs text-foreground">
                  {`kubectl create token rustfs-operator \\
  -n rustfs-system \\
  --duration=24h`}
                </pre>
                <p className="text-xs text-muted-foreground">{t("Paste the token above to sign in.")}</p>
              </div>
            )}
          </div>
        </CardContent>
      </Card>
    </div>
  )
}
