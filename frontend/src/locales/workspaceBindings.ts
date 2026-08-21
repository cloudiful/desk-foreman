export interface WorkspaceBindingsMessages {
  actions: {
    refresh: string
    apply: string
    archive: string
    restore: string
    reset: string
  }
  filters: {
    applicationId: string
    externalUserId: string
    workspaceKey: string
    allStates: string
    active: string
    archived: string
    resetting: string
  }
  table: {
    binding: string
    identity: string
    workspace: string
    state: string
    lastUsed: string
  }
  empty: {
    title: string
    description: string
  }
  pagination: string
  confirmations: {
    title: string
    archiveDescription: string
    restoreDescription: string
    resetDescription: string
  }
  notifications: {
    archiveSuccess: string
    restoreSuccess: string
    resetSuccess: string
    binding: string
  }
  errors: {
    load: string
    archive: string
    restore: string
    reset: string
  }
  identity: {
    application: string
  }
}

const workspaceBindings: WorkspaceBindingsMessages = {
  actions: {
    refresh: 'Refresh workspace bindings',
    apply: 'Apply',
    archive: 'Archive',
    restore: 'Restore',
    reset: 'Reset',
  },
  filters: {
    applicationId: 'Application ID',
    externalUserId: 'External user ID',
    workspaceKey: 'Workspace key',
    allStates: 'All states',
    active: 'Active',
    archived: 'Archived',
    resetting: 'Resetting',
  },
  table: {
    binding: 'Binding',
    identity: 'Identity',
    workspace: 'Workspace',
    state: 'State',
    lastUsed: 'Last used',
  },
  empty: {
    title: 'No workspace bindings',
    description: 'Bindings appear when applications access workspaces.',
  },
  pagination: 'Page {page} of {totalPages}',
  confirmations: {
    title: '{action} workspace',
    archiveDescription:
      '{id} is archived: the workspace is detached and stops accepting activity.',
    restoreDescription: '{id} is restored and becomes active again.',
    resetDescription:
      '{id} is reset: the workspace directory is cleared. This cannot be undone.',
  },
  notifications: {
    archiveSuccess: 'Workspace archived',
    restoreSuccess: 'Workspace restored',
    resetSuccess: 'Workspace reset',
    binding: 'Binding #{id}',
  },
  errors: {
    load: 'Failed to load workspace bindings',
    archive: 'Failed to archive workspace',
    restore: 'Failed to restore workspace',
    reset: 'Failed to reset workspace',
  },
  identity: {
    application: 'App',
  },
}

export const zhCN: WorkspaceBindingsMessages = {
  actions: {
    refresh: '刷新工作区绑定',
    apply: '应用',
    archive: '归档',
    restore: '恢复',
    reset: '重置',
  },
  filters: {
    applicationId: '应用 ID',
    externalUserId: '外部用户 ID',
    workspaceKey: '工作区密钥',
    allStates: '所有状态',
    active: '活跃',
    archived: '已归档',
    resetting: '重置中',
  },
  table: {
    binding: '绑定',
    identity: '身份',
    workspace: '工作区',
    state: '状态',
    lastUsed: '最后使用',
  },
  empty: {
    title: '暂无工作区绑定',
    description: '应用访问工作区后，绑定会显示在这里。',
  },
  pagination: '第 {page} 页，共 {totalPages} 页',
  confirmations: {
    title: '{action}工作区',
    archiveDescription: '{id} 将被归档：工作区会解除绑定并停止接受活动。',
    restoreDescription: '{id} 将被恢复并重新变为活跃状态。',
    resetDescription: '{id} 将被重置：工作区目录会被清空，且无法撤销。',
  },
  notifications: {
    archiveSuccess: '工作区已归档',
    restoreSuccess: '工作区已恢复',
    resetSuccess: '工作区已重置',
    binding: '绑定 #{id}',
  },
  errors: {
    load: '加载工作区绑定失败',
    archive: '归档工作区失败',
    restore: '恢复工作区失败',
    reset: '重置工作区失败',
  },
  identity: {
    application: '应用',
  },
}

export default workspaceBindings
