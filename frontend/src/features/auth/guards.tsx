import type { ReactNode } from 'react'
import { Navigate, Outlet, useLocation } from 'react-router'
import { FullPageLoading } from '@/components/layout/full-page-loading'
import { useSession } from './session-context'

export function RequireSession({ children }: { children: ReactNode }) {
  const { user, isLoading } = useSession()
  const location = useLocation()

  if (isLoading) return <FullPageLoading />
  if (!user) {
    return <Navigate to="/login" replace state={{ from: location.pathname }} />
  }
  if (user.must_change_password) {
    return <Navigate to="/change-password" replace />
  }
  return children ?? <Outlet />
}

export function RequireAdmin() {
  const { user } = useSession()

  if (!user?.is_admin) return <Navigate to="/" replace />
  return <Outlet />
}

export function GuestOnly({ children }: { children: ReactNode }) {
  const { user, isLoading } = useSession()

  if (isLoading) return <FullPageLoading />
  if (user) {
    return (
      <Navigate
        to={user.must_change_password ? '/change-password' : '/'}
        replace
      />
    )
  }
  return children
}

export function RequireAuthenticated({ children }: { children: ReactNode }) {
  const { user, isLoading } = useSession()

  if (isLoading) return <FullPageLoading />
  if (!user) return <Navigate to="/login" replace />
  return children
}
