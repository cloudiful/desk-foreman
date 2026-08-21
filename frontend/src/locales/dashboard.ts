export interface DashboardMessages {
  stats: {
    activeRunners: string
    activeSessions: string
    failedOperations: string
    archivedWorkspaces: string
  }
  refresh: string
  runnerManagers: string
  runnerManagersSummary: string
  manage: string
  lastSeen: string
  noRunnerManagers: string
  noRunnerManagersDescription: string
  quickActions: string
  commonTasks: string
  createUser: string
  createApplication: string
  registerRunnerManager: string
}

const dashboard = {
  stats: {
    activeRunners: 'Active runners',
    activeSessions: 'Active sessions',
    failedOperations: 'Failed operations',
    archivedWorkspaces: 'Archived workspaces',
  },
  refresh: 'Refresh',
  runnerManagers: 'Runner managers',
  runnerManagersSummary:
    '{online} online · {offline} offline · {disabled} disabled',
  manage: 'Manage',
  lastSeen: 'Last seen {time}',
  noRunnerManagers: 'No runner managers',
  noRunnerManagersDescription:
    'Register a runner manager to start executing workspace jobs.',
  quickActions: 'Quick actions',
  commonTasks: 'Common administration tasks',
  createUser: 'Create a user',
  createApplication: 'Create an application',
  registerRunnerManager: 'Register a runner manager',
}

export const zhCN: DashboardMessages = {
  stats: {
    activeRunners: '活跃运行器',
    activeSessions: '活跃会话',
    failedOperations: '失败操作',
    archivedWorkspaces: '已归档工作区',
  },
  refresh: '刷新',
  runnerManagers: '运行器管理器',
  runnerManagersSummary:
    '{online} 个在线 · {offline} 个离线 · {disabled} 个已禁用',
  manage: '管理',
  lastSeen: '最后活动于 {time}',
  noRunnerManagers: '暂无运行器管理器',
  noRunnerManagersDescription: '注册运行器管理器以开始执行工作区任务。',
  quickActions: '快捷操作',
  commonTasks: '常用管理任务',
  createUser: '创建用户',
  createApplication: '创建应用',
  registerRunnerManager: '注册运行器管理器',
}

export default dashboard
