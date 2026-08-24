export interface ApplicationsMessages {
  actions: {
    refresh: string
    create: string
    edit: string
    tokens: string
    saveChanges: string
  }
  searchPlaceholder: string
  filters: {
    allStatuses: string
    active: string
    inactive: string
  }
  count: string
  table: {
    application: string
    limits: string
    scopes: string
    status: string
    updated: string
  }
  empty: {
    matchingTitle: string
    title: string
    searchDescription: string
    description: string
  }
  limits: {
    timeout: string
    noTimeout: string
    output: string
    unlimitedOutput: string
    scopes: string
    defaultShell: string
  }
  pagination: string
  drawer: {
    createTitle: string
    editTitle: string
  }
  sections: {
    general: string
    resourceLimits: string
  }
  fields: {
    name: string
    workspaceTemplate: string
    defaultShell: string
    maxTimeout: string
    maxOutput: string
    maxFile: string
    maxSessions: string
    networkAccess: string
    active: string
  }
  hints: {
    workspaceTemplate: string
    networkAccess: string
    active: string
  }
  placeholders: {
    name: string
    workspaceTemplate: string
    defaultShell: string
    unlimited: string
    tokenName: string
  }
  validation: {
    nameRequired: string
  }
  notifications: {
    created: string
    updated: string
  }
  errors: {
    load: string
    save: string
    loadTokens: string
    createToken: string
    revokeToken: string
  }
  tokens: {
    title: string
    noTokens: string
  }
  confirmations: {
    revokeTitle: string
    revokeDescription: string
  }
}

const applications: ApplicationsMessages = {
  actions: {
    refresh: 'Refresh',
    create: 'Create application',
    edit: 'Edit application',
    tokens: 'Application tokens',
    saveChanges: 'Save changes',
  },
  searchPlaceholder: 'Search applications…',
  filters: {
    allStatuses: 'All statuses',
    active: 'Active',
    inactive: 'Inactive',
  },
  count: '{total} applications',
  table: {
    application: 'Application',
    limits: 'Limits',
    scopes: 'Scopes',
    status: 'Status',
    updated: 'Updated',
  },
  empty: {
    matchingTitle: 'No matching applications',
    title: 'No applications yet',
    searchDescription: 'Try a different search.',
    description: 'Create the first application to get started.',
  },
  limits: {
    timeout: 'timeout {value}',
    noTimeout: 'no timeout',
    output: 'output {value}',
    unlimitedOutput: 'unlimited output',
    scopes: '{count} scopes',
    defaultShell: 'default shell',
  },
  pagination: 'Page {page} of {totalPages}',
  drawer: {
    createTitle: 'Create application',
    editTitle: 'Edit application',
  },
  sections: {
    general: 'General',
    resourceLimits: 'Resource limits',
  },
  fields: {
    name: 'Name',
    workspaceTemplate: 'Workspace template',
    defaultShell: 'Default shell',
    maxTimeout: 'Max timeout (ms)',
    maxOutput: 'Max output (bytes)',
    maxFile: 'Max file (bytes)',
    maxSessions: 'Max sessions',
    networkAccess: 'Network access',
    active: 'Active',
  },
  hints: {
    workspaceTemplate: 'Optional template directory copied into new workspaces',
    networkAccess: 'Allow outbound network for sessions',
    active: 'Inactive applications cannot authenticate',
  },
  placeholders: {
    name: 'e.g. code-agent',
    workspaceTemplate: 'e.g. web-app-template',
    defaultShell: '/bin/bash',
    unlimited: 'Unlimited',
    tokenName: 'e.g. ci-agent',
  },
  validation: {
    nameRequired: 'Name is required',
  },
  notifications: {
    created: 'Application created',
    updated: 'Application updated',
  },
  errors: {
    load: 'Failed to load applications',
    save: 'Failed to save application',
    loadTokens: 'Failed to load application tokens',
    createToken: 'Failed to create token',
    revokeToken: 'Failed to revoke token',
  },
  tokens: {
    title: 'Application tokens',
    noTokens: 'No tokens for this application yet.',
  },
  confirmations: {
    revokeTitle: 'Revoke token',
    revokeDescription:
      'Clients using {tokenName} will lose access immediately.',
  },
}

export const zhCN: ApplicationsMessages = {
  actions: {
    refresh: '刷新',
    create: '创建应用',
    edit: '编辑应用',
    tokens: '应用令牌',
    saveChanges: '保存更改',
  },
  searchPlaceholder: '搜索应用…',
  filters: {
    allStatuses: '所有状态',
    active: '活跃',
    inactive: '未启用',
  },
  count: '共 {total} 个应用',
  table: {
    application: '应用',
    limits: '限制',
    scopes: '权限范围',
    status: '状态',
    updated: '更新时间',
  },
  empty: {
    matchingTitle: '没有匹配的应用',
    title: '暂无应用',
    searchDescription: '请尝试其他搜索条件。',
    description: '创建第一个应用以开始使用。',
  },
  limits: {
    timeout: '超时 {value}',
    noTimeout: '无超时限制',
    output: '输出 {value}',
    unlimitedOutput: '输出不限',
    scopes: '{count} 个权限范围',
    defaultShell: '默认 Shell',
  },
  pagination: '第 {page} 页，共 {totalPages} 页',
  drawer: {
    createTitle: '创建应用',
    editTitle: '编辑应用',
  },
  sections: {
    general: '常规',
    resourceLimits: '资源限制',
  },
  fields: {
    name: '名称',
    workspaceTemplate: '工作区模板',
    defaultShell: '默认 Shell',
    maxTimeout: '最大超时（毫秒）',
    maxOutput: '最大输出（字节）',
    maxFile: '最大文件（字节）',
    maxSessions: '最大会话数',
    networkAccess: '网络访问',
    active: '启用',
  },
  hints: {
    workspaceTemplate: '可选的模板目录，会复制到新工作区中',
    networkAccess: '允许会话访问出站网络',
    active: '未启用的应用无法进行身份验证',
  },
  placeholders: {
    name: '例如 code-agent',
    workspaceTemplate: '例如 web-app-template',
    defaultShell: '/bin/bash',
    unlimited: '不限',
    tokenName: '例如 ci-agent',
  },
  validation: {
    nameRequired: '名称为必填项',
  },
  notifications: {
    created: '应用已创建',
    updated: '应用已更新',
  },
  errors: {
    load: '加载应用失败',
    save: '保存应用失败',
    loadTokens: '加载应用令牌失败',
    createToken: '创建令牌失败',
    revokeToken: '撤销令牌失败',
  },
  tokens: {
    title: '应用令牌',
    noTokens: '此应用暂无令牌。',
  },
  confirmations: {
    revokeTitle: '撤销令牌',
    revokeDescription: '使用 {tokenName} 的客户端将立即失去访问权限。',
  },
}

export default applications
