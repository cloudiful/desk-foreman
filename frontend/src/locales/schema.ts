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
    pageHeader: {
      controlPlane: string
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
