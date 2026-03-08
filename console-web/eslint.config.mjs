import nextVitals from "eslint-config-next/core-web-vitals"
import prettierConfig from "eslint-config-prettier/flat"

/** @type {import('eslint').Linter.Config[]} */
const eslintConfig = [
  ...nextVitals,
  { rules: prettierConfig.rules },
  {
    ignores: [".next/", "out/", "build/", "next-env.d.ts"],
  },
]

export default eslintConfig
