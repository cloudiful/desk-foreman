export interface OperationsMessages {
  actions: {
    refresh: string
  }
  autoRefresh: string
  stats: {
    activeSessions: string
    runningRunners: string
    failedOperations: string
  }
  tabs: {
    runners: string
    sessions: string
  }
  runners: {
    table: {
      runner: string
      owner: string
      status: string
      lastActive: string
    }
    emptyTitle: string
    emptyDescription: string
    detailsTitle: string
    details: {
      container: string
      containerId: string
      runnerManager: string
      status: string
      runtime: string
      image: string
      workspaceRoot: string
      lastActive: string
      lastObserved: string
      lastError: string
    }
    detailsAriaLabel: string
  }
  sessions: {
    table: {
      session: string
      owner: string
      state: string
      exit: string
      duration: string
    }
    emptyTitle: string
    emptyDescription: string
    detailsTitle: string
    details: {
      sessionId: string
      sessionKey: string
      owner: string
      state: string
      exitCode: string
      wallTime: string
      timedOut: string
    }
    detailsAriaLabel: string
  }
  timeout: string
  yes: string
  no: string
  pagination: string
  errors: {
    load: string
    refresh: string
    loadRunners: string
    loadSessions: string
    loadSummary: string
  }
}

const operations: OperationsMessages = {
  actions: {
    refresh: 'Refresh operations',
  },
  autoRefresh: 'Auto-refresh',
  stats: {
    activeSessions: 'Active sessions',
    runningRunners: 'Running runners',
    failedOperations: 'Failed operations',
  },
  tabs: {
    runners: 'Runners ({count})',
    sessions: 'Sessions ({count})',
  },
  runners: {
    table: {
      runner: 'Runner',
      owner: 'Owner',
      status: 'Status',
      lastActive: 'Last active',
    },
    emptyTitle: 'No runners',
    emptyDescription:
      'Runners appear once a workspace command has been executed.',
    detailsTitle: 'Runner details',
    details: {
      container: 'Container',
      containerId: 'Container ID',
      runnerManager: 'Runner manager',
      status: 'Status',
      runtime: 'Runtime',
      image: 'Image',
      workspaceRoot: 'Workspace root',
      lastActive: 'Last active',
      lastObserved: 'Last observed',
      lastError: 'Last error',
    },
    detailsAriaLabel: 'Runner details',
  },
  sessions: {
    table: {
      session: 'Session',
      owner: 'Owner',
      state: 'State',
      exit: 'Exit',
      duration: 'Duration',
    },
    emptyTitle: 'No sessions',
    emptyDescription: 'Sessions appear once a workspace command has run.',
    detailsTitle: 'Session details',
    details: {
      sessionId: 'Session ID',
      sessionKey: 'Session key',
      owner: 'Owner',
      state: 'State',
      exitCode: 'Exit code',
      wallTime: 'Wall time',
      timedOut: 'Timed out',
    },
    detailsAriaLabel: 'Session details',
  },
  timeout: 'timeout',
  yes: 'Yes',
  no: 'No',
  pagination: 'Page {page} of {totalPages}',
  errors: {
    load: 'Failed to load runner status',
    refresh: 'Failed to refresh operations',
    loadRunners: 'Failed to load workspace runners',
    loadSessions: 'Failed to load runner sessions',
    loadSummary: 'Failed to load operations summary',
  },
}

export const zhCN: OperationsMessages = {
  actions: {
    refresh: '刷新操作状态',
  },
  autoRefresh: '自动刷新',
  stats: {
    activeSessions: '活跃会话',
    runningRunners: '运行中的运行器',
    failedOperations: '失败操作',
  },
  tabs: {
    runners: '运行器（{count}）',
    sessions: '会话（{count}）',
  },
  runners: {
    table: {
      runner: '运行器',
      owner: '所有者',
      status: '状态',
      lastActive: '最后活动',
    },
    emptyTitle: '暂无运行器',
    emptyDescription: '执行工作区命令后，运行器会显示在这里。',
    detailsTitle: '运行器详情',
    details: {
      container: '容器',
      containerId: '容器 ID',
      runnerManager: '运行器管理器',
      status: '状态',
      runtime: '运行时',
      image: '镜像',
      workspaceRoot: '工作区根目录',
      lastActive: '最后活动',
      lastObserved: '最后观测',
      lastError: '最后错误',
    },
    detailsAriaLabel: '运行器详情',
  },
  sessions: {
    table: {
      session: '会话',
      owner: '所有者',
      state: '状态',
      exit: '退出',
      duration: '耗时',
    },
    emptyTitle: '暂无会话',
    emptyDescription: '运行工作区命令后，会话会显示在这里。',
    detailsTitle: '会话详情',
    details: {
      sessionId: '会话 ID',
      sessionKey: '会话密钥',
      owner: '所有者',
      state: '状态',
      exitCode: '退出代码',
      wallTime: '墙钟时间',
      timedOut: '是否超时',
    },
    detailsAriaLabel: '会话详情',
  },
  timeout: '超时',
  yes: '是',
  no: '否',
  pagination: '第 {page} 页，共 {totalPages} 页',
  errors: {
    load: '加载运行器状态失败',
    refresh: '刷新操作状态失败',
    loadRunners: '加载工作区运行器失败',
    loadSessions: '加载运行器会话失败',
    loadSummary: '加载操作摘要失败',
  },
}

export default operations
