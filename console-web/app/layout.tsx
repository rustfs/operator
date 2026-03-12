import type { Metadata } from "next"
import Script from "next/script"
import { ThemeProvider } from "next-themes"
import { I18nProvider } from "@/components/providers/i18n-provider"
import { AuthProvider } from "@/contexts/auth-context"
import { AppUiProvider } from "@/components/providers/app-ui-provider"
import "./globals.css"

export const metadata: Metadata = {
  title: "RustFS Operator Console",
  description: "Manage your RustFS tenants and clusters",
}

// Inline script: set <base href> to current origin (protocol + host + /) so relative URLs
// (e.g. /tenants) resolve to the same port as the page. Avoids prefetch/nav
// going to port 80 when the app is actually served on e.g. port 8080 (port-forward).
const setBaseHrefInline = `(function(){var u=location;var b=document.createElement('base');b.href=u.protocol+'//'+u.host+'/';if(document.head.firstChild){document.head.insertBefore(b,document.head.firstChild);}else{document.head.appendChild(b);}})();`

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode
}>) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body className="antialiased">
        <Script id="set-base-href" strategy="beforeInteractive">
          {setBaseHrefInline}
        </Script>
        <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
          <I18nProvider>
            <AuthProvider>
              <AppUiProvider>{children}</AppUiProvider>
            </AuthProvider>
          </I18nProvider>
        </ThemeProvider>
      </body>
    </html>
  )
}
