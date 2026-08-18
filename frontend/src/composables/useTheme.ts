import { computed, ref, watch } from 'vue'

export type ThemeMode = 'dark' | 'light'

const STORAGE_KEY = 'desk-foreman-theme'

function resolveInitial(): ThemeMode {
  if (typeof window === 'undefined') return 'dark'
  try {
    const stored = window.localStorage.getItem(STORAGE_KEY)
    if (stored === 'dark' || stored === 'light') return stored
  } catch {
    // Storage can be unavailable in private browsing or restricted contexts.
  }
  return 'dark'
}

const mode = ref<ThemeMode>(resolveInitial())

function apply(mode: ThemeMode): void {
  const root = document.documentElement
  root.classList.toggle('dark', mode === 'dark')
  root.classList.toggle('light', mode === 'light')
  try {
    window.localStorage.setItem(STORAGE_KEY, mode)
  } catch {
    // Theme still applies for this session when persistence is unavailable.
  }
}

function toggle(): void {
  mode.value = mode.value === 'dark' ? 'light' : 'dark'
}

watch(mode, apply, { immediate: true })

export function useTheme() {
  return {
    mode,
    isDark: computed(() => mode.value === 'dark'),
    toggle,
    apply,
  }
}
