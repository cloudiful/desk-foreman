import type { ApprovalMessages } from './approval'
import type { ApplicationsMessages } from './applications'
import type { AuditMessages } from './audit'
import type { AuthMessages } from './auth'
import type { DashboardMessages } from './dashboard'
import type { OperationsMessages } from './operations'
import type { RunnerManagersMessages } from './runnerManagers'
import type { UsersMessages } from './users'
import type { WorkspaceBindingsMessages } from './workspaceBindings'

export interface MessageSchema {
  app: {
    errorBoundary: {
      title: string
      description: string
      reload: string
    }
  }
  shell: {
    productName: string
    subtitle: string
    navigation: {
      overview: string
      users: string
      applications: string
      runnerManagers: string
      operations: string
      bindings: string
      audit: string
      approval: string
    }
    changePassword: string
    signOut: string
    openNavigation: string
    openUserMenu: string
    switchToLightMode: string
    switchToDarkMode: string
    language: string
    languages: {
      enUS: string
      zhCN: string
    }
    user: string
  }
  auth: AuthMessages
  dashboard: DashboardMessages
  audit: AuditMessages
  approval: ApprovalMessages
  users: UsersMessages
  applications: ApplicationsMessages
  runnerManagers: RunnerManagersMessages
  operations: OperationsMessages
  workspaceBindings: WorkspaceBindingsMessages
  shared: {
    requestFailed: string
    duration: {
      seconds: string
      minutes: string
      hours: string
    }
    confirm: {
      cancel: string
      confirm: string
    }
    dataTable: {
      noData: string
      nothingToShow: string
      loading: string
    }
    empty: {
      noData: string
    }
    errorAlert: {
      description: string
      retry: string
    }
    token: {
      copyAriaLabel: string
      copied: string
      create: string
      revoke: string
      createTitle: string
      copyTitle: string
      copyDescription: string
      active: string
      created: string
      expires: string
      noScopes: string
      pagination: string
      tokenName: string
      scopes: string
      expiresAt: string
      optional: string
      revokeAriaLabel: string
      createdNotification: string
      revokedNotification: string
    }
    notifications: {
      copied: string
      copyFailed: string
    }
    status: {
      online: string
      offline: string
      active: string
      inactive: string
      disabled: string
      success: string
      failure: string
      failed: string
      error: string
      running: string
      pending: string
      idle: string
      stale: string
      archived: string
      cleanup_failed: string
      removed: string
      cancelled: string
      timed_out: string
      exited: string
      resetting: string
      ready: string
      unknown: string
    }
  }
}
