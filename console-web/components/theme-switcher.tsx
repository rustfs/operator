"use client"

import { useTheme } from "next-themes"
import { RiSunFill, RiMoonFill, RiContrast2Line } from "@remixicon/react"
import { Button } from "@/components/ui/button"
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu"
import { useTranslation } from "react-i18next"

const themeOptions = [
  { labelKey: "Light", key: "light" as const, Icon: RiSunFill },
  { labelKey: "Dark", key: "dark" as const, Icon: RiMoonFill },
  { labelKey: "Auto", key: "system" as const, Icon: RiContrast2Line },
]

export function ThemeSwitcher() {
  const { t } = useTranslation()
  const { theme, setTheme } = useTheme()

  const themeIconMap = {
    dark: RiMoonFill,
    light: RiSunFill,
    system: RiContrast2Line,
  } as const
  const themeKey = (theme ?? "system") as keyof typeof themeIconMap
  const Icon = themeIconMap[themeKey] ?? RiContrast2Line

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm" aria-label={t("Light")}>
          <Icon className="size-4 shrink-0" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent className="w-40" align="end">
        {themeOptions.map(({ labelKey, key, Icon: OptionIcon }) => (
          <DropdownMenuItem key={key} onSelect={() => setTheme(key)}>
            <OptionIcon className="me-2 size-4" />
            {t(labelKey)}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
