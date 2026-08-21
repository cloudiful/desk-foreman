import type { ApprovalMessages } from './approval'
import type { AuditMessages } from './audit'
import type { AuthMessages } from './auth'
import type { DashboardMessages } from './dashboard'

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
  shared: {
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
    }
    notifications: {
      copied: string
      copyFailed: string
    }
  }
}
