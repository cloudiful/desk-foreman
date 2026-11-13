import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { useMemo } from 'react'
import { I18nextProvider } from 'react-i18next'
import { RouterProvider } from 'react-router'
import { Toaster } from 'sonner'
import { FullPageLoading } from '@/components/layout/full-page-loading'
import { configureHttpClient } from '@/api/http'
import { AuthProvider } from '@/features/auth/auth-provider'
import { useSession } from '@/features/auth/session-context'
import { i18n } from '@/i18n'
import { ThemeProvider } from '@/lib/theme'
import { router } from '@/router'

configureHttpClient()

function AppRoutes() {
  const { isLoading } = useSession()

  if (isLoading) return <FullPageLoading />
  return <RouterProvider router={router} />
}

export function App() {
  const queryClient = useMemo(
    () =>
      new QueryClient({
        defaultOptions: {
          queries: {
            refetchOnWindowFocus: false,
            retry: false,
          },
        },
      }),
    [],
  )

  return (
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={queryClient}>
        <ThemeProvider>
          <AuthProvider>
            <AppRoutes />
            <Toaster position="bottom-right" richColors closeButton />
          </AuthProvider>
        </ThemeProvider>
      </QueryClientProvider>
    </I18nextProvider>
  )
}
