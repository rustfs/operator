# RustFS Console 前端编程参考文档

> 本文档是基于 `rustfs/console` 项目的完整技术分析，用于指导 Cursor AI 在创建新项目时保持一致的技术选型、设计风格和编码规范。新项目与当前项目属于同一组织，UI 风格和颜色体系必须保持统一。

---

## 目录

1. [技术栈概览](#1-技术栈概览)
2. [项目初始化模板](#2-项目初始化模板)
3. [设计系统与主题](#3-设计系统与主题)
4. [UI 组件库](#4-ui-组件库)
5. [项目结构规范](#5-项目结构规范)
6. [编码规范与格式化](#6-编码规范与格式化)
7. [布局模式](#7-布局模式)
8. [页面编写模式](#8-页面编写模式)
9. [数据表格模式](#9-数据表格模式)
10. [表单模式](#10-表单模式)
11. [反馈系统（Toast 与 Dialog）](#11-反馈系统toast-与-dialog)
12. [国际化 (i18n)](#12-国际化-i18n)
13. [Hooks 模式](#13-hooks-模式)
14. [Context 状态管理模式](#14-context-状态管理模式)
15. [错误处理模式](#15-错误处理模式)
16. [图标系统](#16-图标系统)
17. [关键设计原则](#17-关键设计原则)

---

## 1. 技术栈概览

### 核心框架

| 技术 | 版本 | 用途 |
|------|------|------|
| **Next.js** | 16.x | App Router, React Server Components |
| **React** | 19.x | UI 框架 |
| **TypeScript** | 5.x | 类型系统 |
| **Tailwind CSS** | 4.x | 原子化 CSS (使用 `@theme inline` 配置) |
| **shadcn/ui** | 3.x | UI 组件库 (style: `radix-lyra`) |
| **Radix UI** | 1.4.x | 无障碍基础组件 |

### UI 与样式

| 库 | 用途 |
|----|------|
| `class-variance-authority` | 组件变体管理 (CVA) |
| `tailwind-merge` | Tailwind 类名合并与冲突解决 |
| `clsx` | 条件类名拼接 |
| `tw-animate-css` | CSS 动画 |
| `next-themes` | 深色/浅色主题切换 |
| `@remixicon/react` | 图标库 (Remix Icon) |

### 数据与表格

| 库 | 用途 |
|----|------|
| `@tanstack/react-table` | 数据表格核心 |
| `@tanstack/react-virtual` | 虚拟滚动 |
| `recharts` | 图表 |

### 表单与交互

| 库 | 用途 |
|----|------|
| `cmdk` | 命令面板 / Combobox |
| `sonner` | Toast 通知 |
| `vaul` | Drawer 抽屉组件 |
| `react-day-picker` | 日期选择器 |
| `embla-carousel-react` | 轮播 |
| `react-resizable-panels` | 可调节面板 |
| `input-otp` | OTP 输入 |

### 国际化

| 库 | 用途 |
|----|------|
| `i18next` | i18n 核心 |
| `react-i18next` | React 绑定 |
| `i18next-browser-languagedetector` | 语言自动检测 |

### 工具

| 库 | 用途 |
|----|------|
| `date-fns` / `dayjs` | 日期处理 |
| `ufo` | URL 处理 |
| `file-saver` | 文件下载 |
| `jszip` | ZIP 压缩 |

### 包管理

- **包管理器**: `pnpm@10.19.0`
- 使用 `pnpm-workspace.yaml` (monorepo 支持)

---

## 2. 项目初始化模板

### package.json 核心脚本

```json
{
  "scripts": {
    "dev": "next dev",
    "build": "next build",
    "start": "next start",
    "lint": "eslint",
    "lint:fix": "eslint --fix",
    "type-check": "tsc --noEmit",
    "format": "prettier --write .",
    "format:check": "prettier --check ."
  }
}
```

### tsconfig.json

```json
{
  "compilerOptions": {
    "target": "ES2017",
    "lib": ["dom", "dom.iterable", "esnext"],
    "allowJs": true,
    "skipLibCheck": true,
    "strict": true,
    "noEmit": true,
    "esModuleInterop": true,
    "module": "esnext",
    "moduleResolution": "bundler",
    "resolveJsonModule": true,
    "isolatedModules": true,
    "jsx": "react-jsx",
    "incremental": true,
    "plugins": [{ "name": "next" }],
    "paths": {
      "@/*": ["./*"]
    }
  },
  "include": [
    "next-env.d.ts",
    "**/*.ts",
    "**/*.tsx",
    "**/*.d.ts",
    ".next/types/**/*.ts",
    ".next/dev/types/**/*.ts",
    "**/*.mts"
  ],
  "exclude": ["node_modules"]
}
```

### next.config.ts

```typescript
import type { NextConfig } from "next"

const nextConfig: NextConfig = {
  basePath: process.env.NEXT_PUBLIC_BASE_PATH ?? "/your-app/path",
}

export default nextConfig
```

### postcss.config.mjs

```javascript
const config = {
  plugins: {
    "@tailwindcss/postcss": {},
  },
}
export default config
```

### components.json (shadcn 配置)

```json
{
  "$schema": "https://ui.shadcn.com/schema.json",
  "style": "radix-lyra",
  "rsc": true,
  "tsx": true,
  "tailwind": {
    "config": "",
    "css": "app/globals.css",
    "baseColor": "neutral",
    "cssVariables": true,
    "prefix": ""
  },
  "iconLibrary": "remixicon",
  "rtl": false,
  "aliases": {
    "components": "@/components",
    "utils": "@/lib/utils",
    "ui": "@/components/ui",
    "lib": "@/lib",
    "hooks": "@/hooks"
  },
  "menuColor": "default",
  "menuAccent": "subtle"
}
```

---

## 3. 设计系统与主题

### 核心设计理念

- **锐利、极简的设计**: 零圆角 (`--radius: 0`)，干净的线条
- **紧凑的排版**: 主文本尺寸为 `text-xs`
- **OKLCH 色彩空间**: 使用现代感知均匀色彩系统
- **完整的深色模式**: 每个颜色变量都有对应的深色值
- **中性色调**: 基色为 `neutral`，无彩色主色调

### globals.css 完整模板

```css
@import "tailwindcss";
@import "tw-animate-css";
@import "shadcn/tailwind.css";

@custom-variant dark (&:is(.dark *));

@theme inline {
  --color-background: var(--background);
  --color-foreground: var(--foreground);
  --font-sans: var(--font-sans);
  --font-mono: var(--font-geist-mono);
  --color-sidebar-ring: var(--sidebar-ring);
  --color-sidebar-border: var(--sidebar-border);
  --color-sidebar-accent-foreground: var(--sidebar-accent-foreground);
  --color-sidebar-accent: var(--sidebar-accent);
  --color-sidebar-primary-foreground: var(--sidebar-primary-foreground);
  --color-sidebar-primary: var(--sidebar-primary);
  --color-sidebar-foreground: var(--sidebar-foreground);
  --color-sidebar: var(--sidebar);
  --color-chart-5: var(--chart-5);
  --color-chart-4: var(--chart-4);
  --color-chart-3: var(--chart-3);
  --color-chart-2: var(--chart-2);
  --color-chart-1: var(--chart-1);
  --color-ring: var(--ring);
  --color-input: var(--input);
  --color-border: var(--border);
  --color-destructive: var(--destructive);
  --color-accent-foreground: var(--accent-foreground);
  --color-accent: var(--accent);
  --color-muted-foreground: var(--muted-foreground);
  --color-muted: var(--muted);
  --color-secondary-foreground: var(--secondary-foreground);
  --color-secondary: var(--secondary);
  --color-primary-foreground: var(--primary-foreground);
  --color-primary: var(--primary);
  --color-popover-foreground: var(--popover-foreground);
  --color-popover: var(--popover);
  --color-card-foreground: var(--card-foreground);
  --color-card: var(--card);
  --radius-sm: calc(var(--radius) - 4px);
  --radius-md: calc(var(--radius) - 2px);
  --radius-lg: var(--radius);
  --radius-xl: calc(var(--radius) + 4px);
  --radius-2xl: calc(var(--radius) + 8px);
  --radius-3xl: calc(var(--radius) + 12px);
  --radius-4xl: calc(var(--radius) + 16px);
}

/* ===== 浅色主题 ===== */
:root {
  --background: oklch(1 0 0);           /* 纯白 */
  --foreground: oklch(0.145 0 0);       /* 近黑 */
  --card: oklch(1 0 0);
  --card-foreground: oklch(0.145 0 0);
  --popover: oklch(1 0 0);
  --popover-foreground: oklch(0.145 0 0);
  --primary: oklch(0.205 0 0);          /* 深灰/黑 */
  --primary-foreground: oklch(0.985 0 0);
  --secondary: oklch(0.97 0 0);         /* 极浅灰 */
  --secondary-foreground: oklch(0.205 0 0);
  --muted: oklch(0.97 0 0);
  --muted-foreground: oklch(0.556 0 0); /* 中灰 */
  --accent: oklch(0.97 0 0);
  --accent-foreground: oklch(0.205 0 0);
  --destructive: oklch(0.58 0.22 27);   /* 红/橙色 */
  --border: oklch(0.922 0 0);           /* 浅灰边框 */
  --input: oklch(0.922 0 0);
  --ring: oklch(0.708 0 0);
  --chart-1: oklch(0.809 0.105 251.813);  /* 蓝紫色系图表 */
  --chart-2: oklch(0.623 0.214 259.815);
  --chart-3: oklch(0.546 0.245 262.881);
  --chart-4: oklch(0.488 0.243 264.376);
  --chart-5: oklch(0.424 0.199 265.638);
  --radius: 0;                          /* ⚠️ 关键：零圆角 */
  --sidebar: oklch(0.985 0 0);
  --sidebar-foreground: oklch(0.145 0 0);
  --sidebar-primary: oklch(0.205 0 0);
  --sidebar-primary-foreground: oklch(0.985 0 0);
  --sidebar-accent: oklch(0.97 0 0);
  --sidebar-accent-foreground: oklch(0.205 0 0);
  --sidebar-border: oklch(0.922 0 0);
  --sidebar-ring: oklch(0.708 0 0);
}

/* ===== 深色主题 ===== */
.dark {
  --background: oklch(0.145 0 0);       /* 近黑 */
  --foreground: oklch(0.985 0 0);       /* 近白 */
  --card: oklch(0.205 0 0);
  --card-foreground: oklch(0.985 0 0);
  --popover: oklch(0.205 0 0);
  --popover-foreground: oklch(0.985 0 0);
  --primary: oklch(0.87 0 0);           /* 浅灰 */
  --primary-foreground: oklch(0.205 0 0);
  --secondary: oklch(0.269 0 0);
  --secondary-foreground: oklch(0.985 0 0);
  --muted: oklch(0.269 0 0);
  --muted-foreground: oklch(0.708 0 0);
  --accent: oklch(0.371 0 0);
  --accent-foreground: oklch(0.985 0 0);
  --destructive: oklch(0.704 0.191 22.216);
  --border: oklch(1 0 0 / 10%);         /* 白色 10% 透明度 */
  --input: oklch(1 0 0 / 15%);
  --ring: oklch(0.556 0 0);
  --chart-1: oklch(0.809 0.105 251.813);
  --chart-2: oklch(0.623 0.214 259.815);
  --chart-3: oklch(0.546 0.245 262.881);
  --chart-4: oklch(0.488 0.243 264.376);
  --chart-5: oklch(0.424 0.199 265.638);
  --sidebar: oklch(0.205 0 0);
  --sidebar-foreground: oklch(0.985 0 0);
  --sidebar-primary: oklch(0.488 0.243 264.376);  /* 紫色 */
  --sidebar-primary-foreground: oklch(0.985 0 0);
  --sidebar-accent: oklch(0.269 0 0);
  --sidebar-accent-foreground: oklch(0.985 0 0);
  --sidebar-border: oklch(1 0 0 / 10%);
  --sidebar-ring: oklch(0.556 0 0);
}

@layer base {
  * {
    @apply border-border outline-ring/50;
  }
  body {
    @apply bg-background text-foreground;
  }
}
```

### 色彩体系总结

| 语义 | 浅色模式 | 深色模式 | 用途 |
|------|---------|---------|------|
| `background` | 纯白 | 近黑 | 页面背景 |
| `foreground` | 近黑 | 近白 | 主文本色 |
| `primary` | 深灰/黑 | 浅灰 | 主要按钮、强调 |
| `secondary` | 极浅灰 | 深灰 | 次要按钮 |
| `muted` | 极浅灰 | 深灰 | 弱化区域 |
| `accent` | 极浅灰 | 中深灰 | 高亮/悬停 |
| `destructive` | 红橙色 | 亮红橙 | 危险操作 |
| `border` | 浅灰 | 白色10%透明 | 边框 |
| `chart-*` | 蓝紫色系渐变 | 同浅色 | 图表配色 |

### 关键视觉特征

1. **零圆角** (`--radius: 0`): 所有组件使用 `rounded-none`
2. **小号文字**: 基准文字 `text-xs`（12px）
3. **中性配色**: 无彩色主色调，黑白灰为主
4. **紧凑间距**: 组件高度 `h-8` 为默认
5. **深色模式边框**: 使用透明度而非纯色 (`oklch(1 0 0 / 10%)`)

---

## 4. UI 组件库

### shadcn/ui 组件清单 (58个)

在新项目中安装以下组件（按需选取）：

```
accordion, alert-dialog, alert, aspect-ratio, avatar, badge, breadcrumb,
button-group, button, calendar, card, carousel, chart, checkbox, collapsible,
combobox, command, context-menu, dialog, direction, drawer, dropdown-menu,
empty, field, flip-words, hover-card, input-group, input-otp, input, item,
kbd, label, menubar, native-select, navigation-menu, pagination, popover,
progress, radio-group, resizable, scroll-area, select, separator, sheet,
sidebar, skeleton, slider, sonner, spinner, switch, table, tabs, textarea,
toggle-group, toggle, tooltip
```

### 安装 shadcn 组件命令

```bash
# 初始化 shadcn
pnpm dlx shadcn@latest init

# 安装常用组件
pnpm dlx shadcn@latest add button card dialog input table tabs badge
pnpm dlx shadcn@latest add dropdown-menu select checkbox radio-group
pnpm dlx shadcn@latest add sidebar breadcrumb separator tooltip
pnpm dlx shadcn@latest add alert-dialog sheet popover command
```

### 组件通用模式

所有 UI 组件遵循以下规范：

```typescript
// 1. data-slot 属性标识
<div data-slot="card">

// 2. CVA 变体管理
const buttonVariants = cva("base-classes", {
  variants: {
    variant: { default: "...", outline: "...", ghost: "..." },
    size: { default: "h-8", sm: "h-7", lg: "h-9", icon: "size-8" },
  },
  defaultVariants: { variant: "default", size: "default" },
})

// 3. cn() 合并类名
<button className={cn(buttonVariants({ variant, size }), className)} />

// 4. Radix Slot 组合模式
{asChild ? <Slot.Root>{children}</Slot.Root> : <button>{children}</button>}
```

### Button 变体

| 变体 | 样式 | 用途 |
|------|------|------|
| `default` | `bg-primary text-primary-foreground` | 主要操作 |
| `outline` | 透明背景 + 边框 | 次要操作 |
| `secondary` | `bg-secondary` | 辅助操作 |
| `ghost` | 透明，hover 有底色 | 工具栏按钮 |
| `destructive` | 红色 | 删除、危险操作 |
| `link` | 下划线文本 | 链接样式 |

### Button 尺寸

| 尺寸 | 高度 | 用途 |
|------|------|------|
| `xs` | `h-6` | 紧凑内联按钮 |
| `sm` | `h-7` | 小型按钮 |
| `default` | `h-8` | 默认按钮 |
| `lg` | `h-9` | 强调按钮 |
| `icon` | `size-8` | 图标按钮 |

---

## 5. 项目结构规范

```
your-project/
├── app/                          # Next.js App Router
│   ├── (auth)/                   # 认证路由组
│   │   ├── auth/login/page.tsx
│   │   └── layout.tsx
│   ├── (dashboard)/              # 仪表盘路由组
│   │   ├── _components/          # 布局私有组件
│   │   ├── feature-a/page.tsx    # 各功能页面
│   │   ├── feature-b/page.tsx
│   │   └── layout.tsx
│   ├── globals.css               # 全局样式 + 主题变量
│   ├── layout.tsx                # 根布局（Provider 层）
│   └── favicon.ico
├── assets/                       # 静态资源（SVG logo 等）
├── components/                   # 共享组件
│   ├── ui/                       # shadcn 基础组件（勿直接修改）
│   ├── data-table/               # DataTable 组件
│   ├── providers/                # Provider 包装组件
│   ├── feature-a/                # 领域组件（按功能分组）
│   ├── feature-b/
│   ├── page.tsx                  # 页面包装组件
│   ├── page-header.tsx           # 页面头部
│   ├── empty-state.tsx           # 空状态
│   └── search-input.tsx          # 搜索输入
├── config/                       # 配置文件
│   └── navs.ts                   # 导航配置
├── contexts/                     # React Context
│   ├── auth-context.tsx
│   └── api-context.tsx
├── hooks/                        # 自定义 Hooks
│   ├── use-data-table.tsx
│   ├── use-permissions.tsx
│   └── use-local-storage.ts
├── i18n/
│   └── locales/                  # 语言包 (JSON)
│       ├── en-US.json
│       └── zh-CN.json
├── lib/                          # 工具库
│   ├── feedback/                 # 反馈系统
│   │   ├── dialog.tsx            # 命令式 Dialog API
│   │   └── message.tsx           # 命令式 Toast API
│   ├── api-client.ts
│   ├── config.ts
│   ├── error-handler.ts
│   ├── i18n.ts
│   ├── routes.ts
│   └── utils.ts
├── public/                       # 静态公共资源
├── types/                        # TypeScript 类型定义
├── components.json               # shadcn 配置
├── next.config.ts
├── tsconfig.json
├── package.json
├── postcss.config.mjs
├── .prettierrc
└── eslint.config.mjs
```

### 命名规范

| 类别 | 规范 | 示例 |
|------|------|------|
| **组件文件** | kebab-case | `new-form.tsx`, `page-header.tsx` |
| **组件名** | PascalCase | `NewForm`, `PageHeader` |
| **领域目录** | 复数形式 | `buckets/`, `users/`, `policies/` |
| **文件名不重复目录名** | 简短 | `buckets/info.tsx` (非 `bucket-info.tsx`) |
| **表单文件** | 按类型命名 | `new-form.tsx`, `edit-form.tsx`, `form.tsx` |
| **选择器** | 固定命名 | `selector.tsx` |
| **信息展示** | 固定命名 | `info.tsx` |
| **列表** | 固定命名 | `list.tsx` |
| **Tab 内容** | 后缀 `-tab` | `events-tab.tsx`, `lifecycle-tab.tsx` |

---

## 6. 编码规范与格式化

### Prettier 配置 (`.prettierrc`)

```json
{
  "semi": false,
  "singleQuote": false,
  "jsxSingleQuote": false,
  "trailingComma": "all",
  "printWidth": 120,
  "tabWidth": 2,
  "arrowParens": "always",
  "bracketSpacing": true,
  "quoteProps": "as-needed",
  "bracketSameLine": false,
  "endOfLine": "lf"
}
```

**重点规则**:
- **无分号** (`"semi": false`)
- **双引号** (非单引号)
- **120 字符行宽**
- **尾随逗号** (所有位置)
- **2 空格缩进**

### ESLint 配置 (`eslint.config.mjs`)

```javascript
import { dirname } from "path"
import { fileURLToPath } from "url"
import { FlatCompat } from "@eslint/eslintrc"

const __filename = fileURLToPath(import.meta.url)
const __dirname = dirname(__filename)
const compat = new FlatCompat({ baseDirectory: __dirname })

const eslintConfig = [
  ...compat.extends("next/core-web-vitals", "next/typescript", "prettier"),
  {
    ignores: [".next/", "out/", "build/", "next-env.d.ts"],
  },
]

export default eslintConfig
```

---

## 7. 布局模式

### 根布局 Provider 层级

```tsx
// app/layout.tsx
export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body className={`${fontSans.variable} ${fontMono.variable} antialiased`}>
        <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
          <I18nProvider>
            <AuthProvider>
              <ApiProvider>
                <TaskProvider>
                  <PermissionsProvider>
                    <AppUiProvider>
                      {children}
                    </AppUiProvider>
                  </PermissionsProvider>
                </TaskProvider>
              </ApiProvider>
            </AuthProvider>
          </I18nProvider>
        </ThemeProvider>
      </body>
    </html>
  )
}
```

### Provider 层级说明

```
ThemeProvider          → 主题 (next-themes)
  └─ I18nProvider      → 国际化初始化
    └─ AuthProvider    → 认证状态 (独立，无依赖)
      └─ ApiProvider   → API 客户端 (依赖 Auth)
        └─ TaskProvider → 任务队列 (独立)
          └─ PermissionsProvider → 权限 (依赖 Auth + API)
            └─ AppUiProvider    → UI 反馈系统 (Message + Dialog)
```

### Dashboard 布局

```tsx
// app/(dashboard)/layout.tsx
export default function DashboardLayout({ children }) {
  return (
    <AuthGuard>
      <SidebarProvider>
        <AppSidebar />
        <SidebarInset>
          <div className="flex flex-1 flex-col gap-4 p-6 pt-0">
            <AppTopNav />
            {children}
          </div>
        </SidebarInset>
      </SidebarProvider>
    </AuthGuard>
  )
}
```

### AuthGuard 模式

```tsx
function DashboardAuthGuard({ children }: { children: React.ReactNode }) {
  const { isAuthenticated } = useAuth()
  const { apiReady } = useApiReady()
  const { canAccessPath } = usePermissions()
  const pathname = usePathname()

  // 未认证 → 重定向到登录
  if (!isAuthenticated) {
    redirect("/auth/login?unauthorized=true")
  }

  // 无权限 → 重定向到 403
  if (!canAccessPath(pathname)) {
    redirect("/403")
  }

  // 加载中 → 返回 null
  if (!apiReady) return null

  return <>{children}</>
}
```

### 导航配置模式

```typescript
// config/navs.ts
interface NavItem {
  label: string        // i18n key
  to: string          // 路由路径
  icon: string        // Remix Icon 名称
  isAdminOnly?: boolean
  target?: "_blank"
  type?: "divider"
  children?: NavItem[]
}

export const navItems: NavItem[] = [
  { label: "Browser", to: "/browser", icon: "ri-folder-line" },
  { label: "Access Keys", to: "/access-keys", icon: "ri-key-line" },
  { type: "divider", label: "", to: "" },
  { label: "Users", to: "/users", icon: "ri-user-line", isAdminOnly: true },
  // ...
]
```

---

## 8. 页面编写模式

### 标准 Dashboard 页面结构

```tsx
"use client"

// 1. 导入（按类别分组）
import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { type ColumnDef } from "@tanstack/react-table"

import { Page } from "@/components/page"
import { PageHeader } from "@/components/page-header"
import { DataTable } from "@/components/data-table/data-table"
import { DataTablePagination } from "@/components/data-table/data-table-pagination"
import { Button } from "@/components/ui/button"
import { useDataTable } from "@/hooks/use-data-table"
import { useMessage } from "@/lib/feedback/message"
import { useDialog } from "@/lib/feedback/dialog"
import { useApi } from "@/contexts/api-context"

// 2. 类型定义
interface RowData {
  id: string
  name: string
  // ...
}

// 3. 页面组件
export default function FeaturePage() {
  // (a) Hooks
  const { t } = useTranslation()
  const message = useMessage()
  const dialog = useDialog()
  const api = useApi()

  // (b) 状态
  const [data, setData] = useState<RowData[]>([])
  const [loading, setLoading] = useState(false)

  // (c) 数据获取
  const loadData = async () => {
    setLoading(true)
    try {
      const result = await api.get("/endpoint")
      setData(result)
    } catch (error) {
      message.error(t("Failed to load data"))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    loadData()
  }, [])

  // (d) 列定义
  const columns: ColumnDef<RowData>[] = [
    {
      accessorKey: "name",
      header: t("Name"),
    },
    {
      id: "actions",
      cell: ({ row }) => (
        <Button variant="ghost" size="icon" onClick={() => handleDelete(row.original)}>
          <RiDeleteBinLine className="size-4" />
        </Button>
      ),
    },
  ]

  // (e) DataTable hook
  const { table, selectedRowIds } = useDataTable({
    data,
    columns,
    getRowId: (row) => row.id,
    enableRowSelection: true,
  })

  // (f) 事件处理
  const handleDelete = (item: RowData) => {
    dialog.error({
      title: t("Delete Item"),
      content: t("Are you sure you want to delete this item?"),
      positiveText: t("Delete"),
      negativeText: t("Cancel"),
      onPositiveClick: async () => {
        await api.delete(`/endpoint/${item.id}`)
        message.success(t("Delete Success"))
        loadData()
      },
    })
  }

  // (g) 渲染
  return (
    <Page>
      <PageHeader
        actions={
          <Button onClick={() => setShowNewForm(true)}>
            {t("Create New")}
          </Button>
        }
      >
        <h1>{t("Feature Title")}</h1>
      </PageHeader>

      <DataTable
        table={table}
        isLoading={loading}
        emptyTitle={t("No Data")}
        emptyDescription={t("Get started by creating a new item.")}
      />
      <DataTablePagination table={table} />
    </Page>
  )
}
```

### Page 与 PageHeader 组件

```tsx
// components/page.tsx - 页面容器
export function Page({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cn("flex flex-col gap-4", className)}>{children}</div>
}

// components/page-header.tsx - 页面头部
export function PageHeader({
  children,
  actions,
  className,
}: {
  children: React.ReactNode
  actions?: React.ReactNode
  className?: string
}) {
  return (
    <div className={cn("sticky top-0 z-10 flex items-center justify-between bg-background py-4", className)}>
      <div>{children}</div>
      <div className="flex items-center gap-2">{actions}</div>
    </div>
  )
}
```

---

## 9. 数据表格模式

### useDataTable Hook

```tsx
import { useDataTable } from "@/hooks/use-data-table"
import { DataTable } from "@/components/data-table/data-table"
import { DataTablePagination } from "@/components/data-table/data-table-pagination"

// 使用
const { table, selectedRows, selectedRowIds } = useDataTable<RowData>({
  data: filteredData,
  columns,
  getRowId: (row) => row.id,
  enableRowSelection: true,        // 可选：启用行选择
  manualPagination: false,         // 可选：手动分页（服务端）
  manualSorting: false,            // 可选：手动排序（服务端）
})

// 渲染
<DataTable
  table={table}
  isLoading={loading}
  emptyTitle={t("No Data")}
  emptyDescription={t("Description text")}
  bodyHeight="calc(100vh - 300px)"  // 可选：固定高度 + 滚动
/>
<DataTablePagination table={table} />
```

### 列定义模式

```tsx
const columns: ColumnDef<RowData>[] = [
  // 文本列
  {
    accessorKey: "name",
    header: t("Name"),
    meta: { width: 200 },  // 可选宽度
  },
  // 自定义渲染列
  {
    accessorKey: "status",
    header: t("Status"),
    cell: ({ row }) => (
      <Badge variant={row.original.status === "active" ? "default" : "secondary"}>
        {row.original.status}
      </Badge>
    ),
  },
  // 操作列
  {
    id: "actions",
    meta: { width: 80 },
    cell: ({ row }) => (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon-sm">
            <RiMoreLine className="size-4" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent>
          <DropdownMenuItem onClick={() => handleEdit(row.original)}>
            {t("Edit")}
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => handleDelete(row.original)}>
            {t("Delete")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    ),
  },
]
```

---

## 10. 表单模式

### Dialog 表单模式

```tsx
"use client"

import { useState } from "react"
import { useTranslation } from "react-i18next"
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Field, FieldLabel, FieldContent, FieldDescription } from "@/components/ui/field"
import { Spinner } from "@/components/ui/spinner"
import { useMessage } from "@/lib/feedback/message"

interface NewFormProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSuccess?: () => void
}

export function NewForm({ open, onOpenChange, onSuccess }: NewFormProps) {
  const { t } = useTranslation()
  const message = useMessage()
  const [loading, setLoading] = useState(false)
  const [formData, setFormData] = useState({
    name: "",
    description: "",
  })

  const handleSubmit = async () => {
    if (!formData.name.trim()) {
      message.warning(t("Name is required"))
      return
    }

    setLoading(true)
    try {
      await api.post("/endpoint", formData)
      message.success(t("Created successfully"))
      onOpenChange(false)
      onSuccess?.()
    } catch (error) {
      message.error(t("Failed to create"))
    } finally {
      setLoading(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t("Create New Item")}</DialogTitle>
        </DialogHeader>

        <div className="flex flex-col gap-4">
          <Field>
            <FieldLabel>{t("Name")}</FieldLabel>
            <FieldContent>
              <Input
                value={formData.name}
                onChange={(e) => setFormData((prev) => ({ ...prev, name: e.target.value }))}
                placeholder={t("Enter name")}
              />
            </FieldContent>
            <FieldDescription>{t("A unique name for the item")}</FieldDescription>
          </Field>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {t("Cancel")}
          </Button>
          <Button onClick={handleSubmit} disabled={loading}>
            {loading && <Spinner className="mr-2" />}
            {t("Create")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
```

### Field 组件结构

```tsx
<Field>
  <FieldLabel>{t("Label")}</FieldLabel>
  <FieldContent>
    <Input /> {/* 或 Select, Checkbox, TextArea 等 */}
  </FieldContent>
  <FieldDescription>{t("Help text")}</FieldDescription>
</Field>
```

---

## 11. 反馈系统（Toast 与 Dialog）

### Toast (Message) 系统

基于 `sonner` 库，提供命令式 API：

```tsx
// 使用
import { useMessage } from "@/lib/feedback/message"

function MyComponent() {
  const message = useMessage()

  // 各类型 toast
  message.success("操作成功")
  message.error("操作失败")
  message.warning("请注意")
  message.info("提示信息")

  // 带描述
  message.success("Created", { description: "Item was created successfully" })

  // loading 状态
  const handle = message.loading("Processing...")
  // ... 操作完成后
  handle.destroy()

  // 清除所有
  message.destroyAll()
}
```

### Dialog 确认系统

命令式对话框 API：

```tsx
import { useDialog } from "@/lib/feedback/dialog"

function MyComponent() {
  const dialog = useDialog()

  // 危险确认
  dialog.error({
    title: "Delete Item",
    content: "This action cannot be undone.",
    positiveText: "Delete",
    negativeText: "Cancel",
    onPositiveClick: async () => {
      await deleteItem()
      // 不返回 false 则自动关闭
    },
  })

  // 警告确认
  dialog.warning({
    title: "Warning",
    content: "Are you sure?",
    positiveText: "Confirm",
    negativeText: "Cancel",
    onPositiveClick: async () => { /* ... */ },
  })

  // 普通确认
  dialog.create({
    title: "Confirm",
    content: "Proceed with this action?",
    positiveText: "OK",
    negativeText: "Cancel",
    onPositiveClick: async () => { /* ... */ },
  })
}
```

### AppUiProvider 集成

```tsx
// components/providers/app-ui-provider.tsx
export function AppUiProvider({ children }: { children: React.ReactNode }) {
  return (
    <MessageProvider>
      <DialogProvider>
        {children}
        <DialogHost />
        <Toaster position="top-center" richColors closeButton />
      </DialogProvider>
    </MessageProvider>
  )
}
```

---

## 12. 国际化 (i18n)

### 初始化配置

```typescript
// lib/i18n.ts
import i18n from "i18next"
import { initReactI18next } from "react-i18next"
import LanguageDetector from "i18next-browser-languagedetector"

// 语言映射
const localeMap = {
  en: () => import("@/i18n/locales/en-US.json"),
  zh: () => import("@/i18n/locales/zh-CN.json"),
  ja: () => import("@/i18n/locales/ja-JP.json"),
  // ...
}

i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    fallbackLng: "en",
    interpolation: {
      escapeValue: false,
      prefix: "{",      // 使用 {variable} 语法
      suffix: "}",
    },
    detection: {
      order: ["cookie", "localStorage", "navigator"],
      lookupCookie: "i18n_redirected",
      lookupLocalStorage: "i18n_redirected",
    },
  })
```

### 语言包格式

```json
{
  "Access Keys": "访问密钥",
  "Create New": "新建",
  "Delete Success": "删除成功",
  "Are you sure you want to delete?": "确定要删除吗？",
  "Add {type} Destination": "添加{type}目标"
}
```

**规则**:
- 扁平 key-value 结构
- key 使用英文原文
- 支持 `{variable}` 插值
- 支持 12 种语言：en, zh, ja, ko, de, fr, es, pt, it, ru, tr, id

### 使用模式

```tsx
import { useTranslation } from "react-i18next"

function MyComponent() {
  const { t } = useTranslation()

  return (
    <div>
      <h1>{t("Page Title")}</h1>
      <p>{t("Add {type} Destination", { type: "Webhook" })}</p>
    </div>
  )
}
```

---

## 13. Hooks 模式

### 自定义 Hook 规范

```typescript
// 文件命名: hooks/use-feature-name.ts
// Hook 命名: useFeatureName

// 1. 基本模式
export function useLocalStorage<T>(key: string, defaultValue: T): [T, (value: T) => void] {
  // SSR 安全检查
  const isClient = typeof window !== "undefined"
  // 状态 + 副作用
  const [value, setValue] = useState<T>(() => {
    if (!isClient) return defaultValue
    try {
      const stored = window.localStorage.getItem(key)
      return stored ? JSON.parse(stored) : defaultValue
    } catch {
      return defaultValue
    }
  })
  // ...
  return [value, setValue]
}

// 2. Context Hook 模式
export function useAuth() {
  const context = useContext(AuthContext)
  if (!context) {
    throw new Error("useAuth must be used within AuthProvider")
  }
  return context
}

// 3. 可选 Context Hook（不抛出错误）
export function useApiOptional() {
  return useContext(ApiContext) ?? null
}

// 4. Ready 状态 Hook
export function useApiReady() {
  const context = useContext(ApiContext)
  return { api: context?.api ?? null, isReady: context?.isReady ?? false }
}
```

### 常用 Hooks 参考

| Hook | 用途 | 返回值 |
|------|------|--------|
| `useDataTable<T>` | 表格状态管理 | `{ table, selectedRows, selectedRowIds }` |
| `usePermissions` | 权限检查 | `{ hasPermission, canAccessPath, isAdmin }` |
| `useLocalStorage<T>` | 本地存储 | `[value, setValue]` |
| `useMobile` | 响应式检测 | `boolean` |
| `useMessage` | Toast API | `{ success, error, warning, info, loading }` |
| `useDialog` | 对话框 API | `{ create, error, warning, info }` |
| `useAuth` | 认证状态 | `{ credentials, login, logout, isAuthenticated }` |
| `useApi` | API 客户端 | `ApiClient` |

---

## 14. Context 状态管理模式

### Context 创建模板

```tsx
"use client"

import { createContext, useContext, useMemo, useState, type ReactNode } from "react"

// 1. 定义类型
interface FeatureContextType {
  data: SomeType | null
  isReady: boolean
  doSomething: (params: Params) => Promise<void>
}

// 2. 创建 Context
const FeatureContext = createContext<FeatureContextType | null>(null)

// 3. Provider
export function FeatureProvider({ children }: { children: ReactNode }) {
  const [data, setData] = useState<SomeType | null>(null)
  const [isReady, setIsReady] = useState(false)

  // 初始化逻辑（通常在 useEffect 中）

  // Memoize value
  const value = useMemo(
    () => ({ data, isReady, doSomething }),
    [data, isReady],
  )

  return <FeatureContext.Provider value={value}>{children}</FeatureContext.Provider>
}

// 4. 必选 Hook（未在 Provider 内使用则抛错）
export function useFeature() {
  const context = useContext(FeatureContext)
  if (!context) {
    throw new Error("useFeature must be used within FeatureProvider")
  }
  return context
}

// 5. 可选 Hook（返回 null）
export function useFeatureOptional() {
  return useContext(FeatureContext)
}

// 6. Ready Hook
export function useFeatureReady() {
  const context = useContext(FeatureContext)
  return { feature: context, isReady: context?.isReady ?? false }
}
```

### Context 依赖链

```
AuthProvider (独立)
  → ApiProvider (依赖 Auth credentials)
    → S3Provider (依赖 Auth + Config)
  → PermissionsProvider (依赖 Auth + API)
TaskProvider (独立)
```

---

## 15. 错误处理模式

### API 错误处理

```typescript
// lib/error-handler.ts
interface ApiError {
  message: string
  code?: string
  statusCode?: number
  originalError?: unknown
}

// 解析 API 响应错误
async function parseApiError(response: Response): Promise<string> {
  try {
    const json = await response.json()
    return json.message || json.error || response.statusText
  } catch {
    const text = await response.text()
    // 尝试解析 XML 错误
    const match = text.match(/<Message>(.*?)<\/Message>/)
    return match?.[1] || response.statusText
  }
}
```

### API 错误处理器

```typescript
// lib/api-error-handler.ts
class ApiErrorHandler {
  constructor(
    private onUnauthorized: () => void,  // 401 → 重定向登录
    private onForbidden: () => void,     // 403 → 重定向 403 页面
    private onServerError?: () => void,  // 500 → 可选处理
  ) {}

  handleByStatus(status: number) {
    switch (status) {
      case 401: return this.onUnauthorized()
      case 403: return this.onForbidden()
      case 500:
      case 502:
      case 503:
      case 504: return this.onServerError?.()
    }
  }
}
```

### 页面级错误处理

```tsx
// 在页面组件中
try {
  const result = await api.get("/endpoint")
  setData(result)
} catch (error) {
  message.error(t("Failed to load data"))
  // API 客户端已处理 401/403 跳转
  // 这里只处理业务级错误提示
}
```

---

## 16. 图标系统

### Remix Icon 使用

```tsx
import { RiAddLine, RiDeleteBinLine, RiEditLine, RiSearchLine } from "@remixicon/react"

// 标准尺寸
<RiAddLine className="size-4" />        // 16px - 按钮/行内图标
<RiDeleteBinLine className="size-3.5" /> // 14px - 小型图标
<RiSearchLine className="size-5" />      // 20px - 输入框图标

// 在 Button 中
<Button>
  <RiAddLine className="size-4" />
  {t("Create")}
</Button>

// 图标按钮
<Button variant="ghost" size="icon">
  <RiDeleteBinLine className="size-4" />
</Button>
```

### 图标映射（导航用）

```tsx
// lib/icon-map.tsx
import { RiFolderLine, RiKeyLine, RiUserLine /* ... */ } from "@remixicon/react"

const iconMap: Record<string, React.ComponentType<{ className?: string }>> = {
  "ri-folder-line": RiFolderLine,
  "ri-key-line": RiKeyLine,
  "ri-user-line": RiUserLine,
  // ...
}

export function getIconComponent(name: string) {
  return iconMap[name]
}
```

### 常用图标参考

- 浏览 Remix Icon: https://remixicon.com/
- 包: `@remixicon/react`
- 命名格式: `Ri{Name}{Style}` (如 `RiAddLine`, `RiDeleteBinFill`)
- 风格: `Line`(线性)、`Fill`(填充)

---

## 17. 关键设计原则

### 必须遵循的视觉规范

1. **零圆角**: 所有组件 `rounded-none`，这是核心视觉特征
2. **紧凑排版**: 默认文字 `text-xs`(12px)，标题可用 `text-sm`(14px)
3. **中性配色**: 黑白灰为主，避免引入彩色主色调
4. **一致的组件高度**: 输入框/按钮默认 `h-8`
5. **OKLCH 色彩空间**: 新增颜色变量使用 OKLCH 格式

### 组件使用规范

1. **不直接修改 `components/ui/`**: 通过 wrapper 组件扩展
2. **使用 `cn()` 合并类名**: `cn(baseClass, conditionalClass, className)`
3. **使用 `data-slot` 属性**: 组件根元素添加标识
4. **CVA 管理变体**: 多变体组件使用 `class-variance-authority`

### 反馈系统使用规范

1. **Toast 用于通知**: `useMessage()` — 操作成功/失败提示
2. **Dialog 用于确认**: `useDialog()` — 危险操作确认
3. **命令式 API**: 非声明式，直接调用方法
4. **统一放在 `lib/feedback/`**: 不在 `components/ui/` 中

### 状态管理规范

1. **Context 用于全局状态**: Auth, API, Permissions
2. **useState 用于页面状态**: 表单数据, loading, 列表数据
3. **useDataTable 用于表格**: 排序、分页、选择
4. **localStorage 用于持久化**: 认证、用户偏好

### 编码规范

1. **所有页面 `"use client"`**: Dashboard 页面为客户端组件
2. **useTranslation() 用于文本**: 不硬编码文字
3. **async/await + try/catch**: 统一异步错误处理
4. **kebab-case 文件名**: `new-form.tsx` 非 `NewForm.tsx`
5. **无分号**: Prettier 配置 `semi: false`
6. **双引号**: Prettier 配置 `singleQuote: false`

---

## 附录：快速启动清单

### 新项目创建步骤

1. **初始化项目**
   ```bash
   pnpm create next-app@latest your-project --typescript --tailwind --app --src-dir=false
   cd your-project
   ```

2. **安装依赖**
   ```bash
   pnpm add class-variance-authority clsx tailwind-merge tw-animate-css
   pnpm add next-themes @remixicon/react sonner vaul
   pnpm add @tanstack/react-table @tanstack/react-virtual
   pnpm add i18next react-i18next i18next-browser-languagedetector
   pnpm add cmdk react-day-picker date-fns dayjs
   pnpm add radix-ui shadcn
   pnpm add -D @tailwindcss/postcss prettier eslint-config-prettier
   ```

3. **初始化 shadcn**
   ```bash
   pnpm dlx shadcn@latest init
   # 选择: style=radix-lyra, baseColor=neutral, iconLibrary=remixicon
   ```

4. **复制配置文件**
   - `.prettierrc` (见第6节)
   - `globals.css` (见第3节)
   - `components.json` (见第2节)
   - `tsconfig.json` (见第2节)

5. **创建目录结构** (见第5节)

6. **安装需要的 shadcn 组件**
   ```bash
   pnpm dlx shadcn@latest add button card dialog input table tabs badge ...
   ```

7. **复制/创建工具文件**
   - `lib/utils.ts` — `cn()` 函数
   - `lib/feedback/message.tsx` — Toast 系统
   - `lib/feedback/dialog.tsx` — Dialog 系统
   - `lib/i18n.ts` — 国际化初始化

8. **搭建 Provider 层级** (见第7节)

9. **开始编写页面** (见第8节)

---

> **注意**: 本文档为参考规范，新项目应根据实际业务需求适当调整，但**设计风格（零圆角、中性配色、紧凑排版）和技术选型必须保持一致**。
