import { createBrowserRouter, Navigate } from 'react-router'
import {
  GuestOnly,
  RequireAdmin,
  RequireAuthenticated,
  RequireSession,
} from '@/features/auth/guards'
import { ChangePasswordPage } from '@/features/auth/change-password-page'
import { LoginPage } from '@/features/auth/login-page'
import { AppShell } from '@/features/shell/app-shell'
import { SettingsPage } from '@/features/settings/settings-page'
import { NotFoundPage } from '@/features/shell/not-found-page'
import { RouteErrorPage } from '@/features/shell/route-error-page'
import { DashboardPage } from '@/features/dashboard/dashboard-page'
import { OperationsPage } from '@/features/operations/operations-page'
import { AuditPage } from '@/features/audit/audit-page'
import { UsersPage } from '@/features/users/users-page'
import { ApplicationsPage } from '@/features/applications/applications-page'
import { RunnerManagersPage } from '@/features/runner-managers/runner-managers-page'
import { WorkspaceBindingsPage } from '@/features/workspace-bindings/workspace-bindings-page'

export const router = createBrowserRouter([
  {
    path: '/login',
    element: (
      <GuestOnly>
        <LoginPage />
      </GuestOnly>
    ),
    errorElement: <RouteErrorPage />,
  },
  {
    path: '/change-password',
    element: (
      <RequireAuthenticated>
        <ChangePasswordPage />
      </RequireAuthenticated>
    ),
    errorElement: <RouteErrorPage />,
  },
  {
    path: '/',
    element: (
      <RequireSession>
        <AppShell />
      </RequireSession>
    ),
    errorElement: <RouteErrorPage />,
    children: [
      { index: true, element: <DashboardPage /> },
      { path: 'settings', element: <SettingsPage /> },
      {
        path: 'admin',
        element: <RequireAdmin />,
        children: [
          { index: true, element: <Navigate to="/admin/users" replace /> },
          { path: 'users', element: <UsersPage /> },
          { path: 'applications', element: <ApplicationsPage /> },
          { path: 'runner-managers', element: <RunnerManagersPage /> },
          { path: 'operations', element: <OperationsPage /> },
          { path: 'workspace-bindings', element: <WorkspaceBindingsPage /> },
          { path: 'audit', element: <AuditPage /> },
        ],
      },
    ],
  },
  { path: '*', element: <NotFoundPage /> },
])
