import { createContext, useContext } from 'react'

export type ThemeMode = 'dark' | 'light'

export interface ThemeContextValue {
  mode: ThemeMode
  isDark: boolean
  toggle: () => void
}

export const ThemeContext = createContext<ThemeContextValue | null>(null)

export function useTheme(): ThemeContextValue {
  const context = useContext(ThemeContext)
  if (!context) {
    throw new Error('useTheme must be used inside ThemeProvider')
  }
  return context
}
