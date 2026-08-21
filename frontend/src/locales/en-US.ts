import type { MessageSchema } from './schema'
import applications from './applications'
import approval from './approval'
import audit from './audit'
import auth from './auth'
import dashboard from './dashboard'
import operations from './operations'
import runnerManagers from './runnerManagers'
import users from './users'
import workspaceBindings from './workspaceBindings'

const enUS = {
  app: {
    errorBoundary: {
      title: 'The page could not be rendered',
      description: 'Reload the page to recover the application.',
      reload: 'Reload',
    },
  },
  shell: {
    productName: 'Desk Foreman',
    subtitle: 'Control plane',
    navigation: {
      overview: 'Overview',
      users: 'Users',
      applications: 'Applications',
      runnerManagers: 'Runner managers',
      operations: 'Operations',
      bindings: 'Bindings',
      audit: 'Audit log',
      approval: 'Approval',
    },
    changePassword: 'Change password',
    signOut: 'Sign out',
    openNavigation: 'Open navigation',
    openUserMenu: 'Open user menu',
    switchToLightMode: 'Switch to light mode',
    switchToDarkMode: 'Switch to dark mode',
    language: 'Language',
    languages: {
      enUS: 'English',
      zhCN: 'Simplified Chinese',
    },
    user: 'User',
  },
  auth,
  dashboard,
  audit,
  approval,
  users,
  applications,
  runnerManagers,
  operations,
  workspaceBindings,
  shared: {
    requestFailed: 'Request failed',
    duration: {
      seconds: '{value}s',
      minutes: '{value}m',
      hours: '{value}h',
    },
    confirm: {
      cancel: 'Cancel',
      confirm: 'Confirm',
    },
    dataTable: {
      noData: 'No data',
      nothingToShow: 'Nothing to show yet.',
      loading: 'Loading…',
    },
    empty: {
      noData: 'No data',
    },
    errorAlert: {
      description: 'The request failed. Check the details and try again.',
      retry: 'Retry',
    },
    token: {
      copyAriaLabel: 'Copy token',
      copied: 'Token copied',
      create: 'Create token',
      revoke: 'Revoke',
      createTitle: 'Create a token',
      copyTitle: 'Copy this token now',
      copyDescription:
        'The full token is shown only once and cannot be retrieved later.',
      active: 'Active tokens',
      created: 'Created {time}',
      expires: 'expires {time}',
      noScopes: 'no scopes',
      pagination: 'Page {page} of {totalPages}',
      tokenName: 'Token name',
      scopes: 'Scopes',
      expiresAt: 'Expires at',
      optional: 'Optional',
      revokeAriaLabel: 'Revoke {tokenName}',
      createdNotification: 'Token created',
      revokedNotification: 'Token revoked',
    },
    notifications: {
      copied: 'Copied',
      copyFailed: 'Copy failed',
    },
    status: {
      online: 'Online',
      offline: 'Offline',
      active: 'Active',
      inactive: 'Inactive',
      disabled: 'Disabled',
      success: 'Success',
      failure: 'Failure',
      failed: 'Failed',
      error: 'Error',
      running: 'Running',
      pending: 'Pending',
      idle: 'Idle',
      stale: 'Stale',
      archived: 'Archived',
      cleanup_failed: 'Cleanup failed',
      removed: 'Removed',
      cancelled: 'Cancelled',
      timed_out: 'Timed out',
      exited: 'Exited',
      resetting: 'Resetting',
      ready: 'Ready',
      unknown: 'Unknown',
    },
  },
} satisfies MessageSchema

export default enUS
