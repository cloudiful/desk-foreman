import type { MessageSchema } from './schema'

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
  shared: {
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
    pageHeader: {
      controlPlane: 'Control plane',
    },
    token: {
      copyAriaLabel: 'Copy token',
      copied: 'Token copied',
    },
    notifications: {
      copied: 'Copied',
      copyFailed: 'Copy failed',
    },
  },
} satisfies MessageSchema

export default enUS
