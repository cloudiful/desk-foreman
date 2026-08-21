export interface ApplicationsMessages {
  actions: {
    refresh: string
    create: string
    edit: string
    tokens: string
    testReviewer: string
    clearApiKey: string
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
    approvalReviewer: string
  }
  fields: {
    name: string
    workspaceTemplate: string
    defaultShell: string
    scopes: string
    maxTimeout: string
    maxOutput: string
    maxFile: string
    maxSessions: string
    networkAccess: string
    active: string
    mode: string
    endpoint: string
    model: string
    apiKey: string
    timeout: string
    maxInput: string
    concurrentReviews: string
    maxOutputTokens: string
  }
  hints: {
    workspaceTemplate: string
    networkAccess: string
    active: string
    apiKey: string
    testReviewer: string
  }
  placeholders: {
    name: string
    workspaceTemplate: string
    defaultShell: string
    unlimited: string
    endpoint: string
    model: string
    apiKey: string
    globalDefault: string
    tokenName: string
  }
  approvalModes: {
    inherit: string
    disabled: string
    enabled: string
  }
  approvalTest: {
    passed: string
    failed: string
    description: string
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
    test: string
    loadTokens: string
    createToken: string
    revokeToken: string
  }
  tokens: {
    title: string
    active: string
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
    testReviewer: 'Test application reviewer',
    clearApiKey: 'Clear application reviewer API key',
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
    approvalReviewer: 'Approval reviewer',
  },
  fields: {
    name: 'Name',
    workspaceTemplate: 'Workspace template',
    defaultShell: 'Default shell',
    scopes: 'Scopes',
    maxTimeout: 'Max timeout (ms)',
    maxOutput: 'Max output (bytes)',
    maxFile: 'Max file (bytes)',
    maxSessions: 'Max sessions',
    networkAccess: 'Network access',
    active: 'Active',
    mode: 'Mode',
    endpoint: 'Responses API base URL',
    model: 'Model',
    apiKey: 'API key',
    timeout: 'Timeout (ms)',
    maxInput: 'Max input (bytes)',
    concurrentReviews: 'Concurrent reviews',
    maxOutputTokens: 'Max output tokens',
  },
  hints: {
    workspaceTemplate: 'Optional template directory copied into new workspaces',
    networkAccess: 'Allow outbound network for sessions',
    active: 'Inactive applications cannot authenticate',
    apiKey: 'Leave blank to keep the stored key',
    testReviewer:
      'Tests the saved application configuration without executing a tool.',
  },
  placeholders: {
    name: 'e.g. code-agent',
    workspaceTemplate: 'e.g. web-app-template',
    defaultShell: '/bin/bash',
    unlimited: 'Unlimited',
    endpoint: 'https://api.openai.com/v1',
    model: 'Reviewer model',
    apiKey: 'Enter an application reviewer key',
    globalDefault: 'Global default',
    tokenName: 'e.g. ci-agent',
  },
  approvalModes: {
    inherit: 'Inherit global settings',
    disabled: 'Disabled',
    enabled: 'Use application reviewer',
  },
  approvalTest: {
    passed: 'Reviewer test passed',
    failed: 'Reviewer test failed',
    description: '{message} ({latency} ms)',
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
    test: 'Failed to test application reviewer',
    loadTokens: 'Failed to load application tokens',
    createToken: 'Failed to create token',
    revokeToken: 'Failed to revoke token',
  },
  tokens: {
    title: 'Application tokens',
    active: 'Active tokens',
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
    testReviewer: '测试应用审查器',
    clearApiKey: '清除应用审查器 API 密钥',
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
    approvalReviewer: '审批审查器',
  },
  fields: {
    name: '名称',
    workspaceTemplate: '工作区模板',
    defaultShell: '默认 Shell',
    scopes: '权限范围',
    maxTimeout: '最大超时（毫秒）',
    maxOutput: '最大输出（字节）',
    maxFile: '最大文件（字节）',
    maxSessions: '最大会话数',
    networkAccess: '网络访问',
    active: '启用',
    mode: '模式',
    endpoint: 'Responses API 基础 URL',
    model: '模型',
    apiKey: 'API 密钥',
    timeout: '超时（毫秒）',
    maxInput: '最大输入（字节）',
    concurrentReviews: '并发审查数',
    maxOutputTokens: '最大输出令牌数',
  },
  hints: {
    workspaceTemplate: '可选的模板目录，会复制到新工作区中',
    networkAccess: '允许会话访问出站网络',
    active: '未启用的应用无法进行身份验证',
    apiKey: '留空以保留已存储的密钥',
    testReviewer: '测试已保存的应用配置，不会执行工具。',
  },
  placeholders: {
    name: '例如 code-agent',
    workspaceTemplate: '例如 web-app-template',
    defaultShell: '/bin/bash',
    unlimited: '不限',
    endpoint: 'https://api.openai.com/v1',
    model: '审查模型',
    apiKey: '输入应用审查器密钥',
    globalDefault: '全局默认值',
    tokenName: '例如 ci-agent',
  },
  approvalModes: {
    inherit: '继承全局设置',
    disabled: '禁用',
    enabled: '使用应用审查器',
  },
  approvalTest: {
    passed: '审查器测试通过',
    failed: '审查器测试失败',
    description: '{message}（{latency} 毫秒）',
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
    test: '测试应用审查器失败',
    loadTokens: '加载应用令牌失败',
    createToken: '创建令牌失败',
    revokeToken: '撤销令牌失败',
  },
  tokens: {
    title: '应用令牌',
    active: '活跃令牌',
    noTokens: '此应用暂无令牌。',
  },
  confirmations: {
    revokeTitle: '撤销令牌',
    revokeDescription: '使用 {tokenName} 的客户端将立即失去访问权限。',
  },
}

export default applications
