import i18n from "i18next"
import { initReactI18next } from "react-i18next"
import LanguageDetector from "i18next-browser-languagedetector"

import enUS from "@/i18n/locales/en-US.json"
import zhCN from "@/i18n/locales/zh-CN.json"

const resources = {
  en: { translation: enUS },
  zh: { translation: zhCN },
}

i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources,
    fallbackLng: "en",
    interpolation: {
      escapeValue: false,
      prefix: "{",
      suffix: "}",
    },
    detection: {
      order: ["cookie", "localStorage", "navigator"],
      lookupCookie: "i18n_redirected",
      lookupLocalStorage: "i18n_redirected",
    },
  })

export default i18n
