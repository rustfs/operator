"use client"

import { useTranslation } from "react-i18next"
import { RiTranslate2 } from "@remixicon/react"
import { Button } from "@/components/ui/button"
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu"

const languageConfig: Record<string, { text: string }> = {
  en: { text: "English" },
  zh: { text: "中文" },
}

const options = Object.entries(languageConfig).map(([key, value]) => ({
  label: value.text,
  key,
}))

export function LanguageSwitcher() {
  const { i18n } = useTranslation()
  const currentLocale = i18n.language?.split("-")[0] ?? "en"
  const currentLanguage = languageConfig[currentLocale] ?? languageConfig.en

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm" aria-label={currentLanguage.text}>
          <RiTranslate2 className="size-4 shrink-0" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent className="w-32" align="end">
        {options.map(({ label, key }) => (
          <DropdownMenuItem key={key} onSelect={() => i18n.changeLanguage(key)}>
            {label}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
