import { ref, watch } from 'vue'
import { createI18n } from 'vue-i18n'
import enUS from './locales/en-US'
import type { MessageSchema } from './locales/schema'
import zhCN from './locales/zh-CN'

export const SUPPORTED_LOCALES = ['en-US', 'zh-CN'] as const
export type Locale = (typeof SUPPORTED_LOCALES)[number]
export const DEFAULT_LOCALE: Locale = 'en-US'

const STORAGE_KEY = 'desk-foreman-locale'

const messages: Record<Locale, MessageSchema> = {
  'en-US': enUS,
  'zh-CN': zhCN,
}

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

export const locale = ref<Locale>(resolveInitialLocale())

export const i18n = createI18n<[MessageSchema], Locale, false>({
  legacy: false,
  locale: locale.value,
  fallbackLocale: DEFAULT_LOCALE,
  messages,
})

function applyLocale(value: Locale): void {
  i18n.global.locale.value = value
  if (typeof document !== 'undefined') {
    document.documentElement.lang = value
  }
  if (typeof window !== 'undefined') {
    try {
      window.localStorage.setItem(STORAGE_KEY, value)
    } catch {
      // Locale still applies for this session when persistence is unavailable.
    }
  }
}

watch(locale, applyLocale, { immediate: true })

export function setLocale(value: Locale): void {
  if (locale.value !== value) locale.value = value
}
