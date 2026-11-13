import i18next from 'i18next'
import { initReactI18next } from 'react-i18next'
import enUS from './locales/en-US'
import zhCN from './locales/zh-CN'

export const SUPPORTED_LOCALES = ['en-US', 'zh-CN'] as const
export type Locale = (typeof SUPPORTED_LOCALES)[number]
export const DEFAULT_LOCALE: Locale = 'en-US'

const STORAGE_KEY = 'desk-foreman-locale'

function isLocale(value: string | null): value is Locale {
  return value !== null && SUPPORTED_LOCALES.includes(value as Locale)
}

function browserLocale(): Locale {
  if (typeof navigator !== 'undefined' && navigator.language.startsWith('zh')) {
    return 'zh-CN'
  }
  return DEFAULT_LOCALE
}

function resolveInitialLocale(): Locale {
  if (typeof window === 'undefined') return DEFAULT_LOCALE

  try {
    const stored = window.localStorage.getItem(STORAGE_KEY)
    if (isLocale(stored)) return stored
  } catch {
    // Browser language remains a useful session-only default when storage is unavailable.
  }

  return browserLocale()
}

export const i18n = i18next.createInstance()

void i18n.use(initReactI18next).init({
  resources: {
    'en-US': { translation: enUS },
    'zh-CN': { translation: zhCN },
  },
  lng: resolveInitialLocale(),
  fallbackLng: DEFAULT_LOCALE,
  initAsync: false,
  interpolation: {
    escapeValue: false,
    prefix: '{',
    suffix: '}',
  },
})

if (typeof document !== 'undefined') {
  document.documentElement.lang = i18n.language
}

export function setLocale(value: Locale): void {
  void i18n.changeLanguage(value)
  if (typeof document !== 'undefined') {
    document.documentElement.lang = value
  }
  if (typeof window === 'undefined') return
  try {
    window.localStorage.setItem(STORAGE_KEY, value)
  } catch {
    // Locale still applies for this session when persistence is unavailable.
  }
}
