import { computed, ref, watch } from 'vue'

export type ThemeMode = 'dark' | 'light'

const STORAGE_KEY = 'desk-foreman-theme'

function resolveInitial(): ThemeMode {
  if (typeof window === 'undefined') return 'dark'
  const stored = window.localStorage.getItem(STORAGE_KEY)
  if (stored === 'dark' || stored === 'light') return stored
  return 'dark'
}

const mode = ref<ThemeMode>(resolveInitial())

function apply(mode: ThemeMode): void {
  const root = document.documentElement
  root.classList.toggle('dark', mode === 'dark')
  root.classList.toggle('light', mode === 'light')
  window.localStorage.setItem(STORAGE_KEY, mode)
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
