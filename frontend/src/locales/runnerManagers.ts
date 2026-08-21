export interface RunnerManagersMessages {
  actions: {
    refresh: string
    add: string
    edit: string
    cancel: string
    register: string
    save: string
    done: string
  }
  stats: {
    online: string
    offline: string
    disabled: string
  }
  searchPlaceholder: string
  table: {
    manager: string
    status: string
    limits: string
    lastSeen: string
  }
  empty: {
    matchingTitle: string
    title: string
    searchDescription: string
    description: string
  }
  limits: string
  pagination: string
  drawer: {
    createTitle: string
    editTitle: string
  }
  sections: {
    connection: string
    container: string
    resourceLimits: string
  }
  fields: {
    name: string
    endpoint: string
    hostWorkspaceRoot: string
    existingToken: string
    enabled: string
    runnerImage: string
    networkAccess: string
    maxSessions: string
    maxOutput: string
    maxTimeout: string
    pidLimit: string
    memoryLimit: string
    cpuLimit: string
  }
  hints: {
    hostWorkspaceRoot: string
    existingToken: string
    enabled: string
    networkAccess: string
    imageAndNetwork: string
    limits: string
  }
  placeholders: {
    name: string
    endpoint: string
    hostWorkspaceRoot: string
    existingToken: string
    runnerImage: string
    memoryLimit: string
    cpuLimit: string
  }
  validation: {
    nameRequired: string
    endpointRequired: string
  }
  notifications: {
    registered: string
    updated: string
  }
  errors: {
    load: string
    save: string
    create: string
    update: string
  }
  token: {
    title: string
    description: string
    copyTitle: string
    copyDescription: string
    configure: string
  }
}

const runnerManagers: RunnerManagersMessages = {
  actions: {
    refresh: 'Refresh runner managers',
    add: 'Add runner manager',
    edit: 'Edit runner manager',
    cancel: 'Cancel',
    register: 'Register manager',
    save: 'Save changes',
    done: 'Done',
  },
  stats: {
    online: 'Online',
    offline: 'Offline',
    disabled: 'Disabled',
  },
  searchPlaceholder: 'Search by name or endpoint…',
  table: {
    manager: 'Manager',
    status: 'Status',
    limits: 'Limits',
    lastSeen: 'Last seen',
  },
  empty: {
    matchingTitle: 'No matching managers',
    title: 'No runner managers',
    searchDescription: 'Try a different search.',
    description: 'Register a runner manager to execute workspace jobs.',
  },
  limits: '{sessions} sessions · {timeout} timeout · {output} output',
  pagination: 'Page {page} of {totalPages}',
  drawer: {
    createTitle: 'Create runner manager',
    editTitle: 'Edit runner manager',
  },
  sections: {
    connection: 'Connection',
    container: 'Container',
    resourceLimits: 'Resource limits',
  },
  fields: {
    name: 'Name',
    endpoint: 'Endpoint',
    hostWorkspaceRoot: 'Host workspace root',
    existingToken: 'Existing token',
    enabled: 'Enabled',
    runnerImage: 'Runner image',
    networkAccess: 'Network access',
    maxSessions: 'Max sessions',
    maxOutput: 'Max output (bytes)',
    maxTimeout: 'Max timeout (ms)',
    pidLimit: 'PID limit',
    memoryLimit: 'Memory limit',
    cpuLimit: 'CPU limit',
  },
  hints: {
    hostWorkspaceRoot:
      'Optional: host path for runner workspace bind mounts; must match the workspace directory mounted into the runner manager',
    existingToken: 'Optional: reuse a previously issued runner token',
    enabled: 'Disabled managers stop receiving jobs',
    networkAccess: 'Allow outbound network in runner containers',
    imageAndNetwork:
      'Image and network changes apply to newly started runner containers only.',
    limits: 'Timeout and output limits are enforced on every job, old and new.',
  },
  placeholders: {
    name: 'e.g. homelab-docker',
    endpoint: 'http://runner-manager:3001',
    hostWorkspaceRoot: '/opt/desk-foreman/vol/workspace',
    existingToken: 'Leave blank to issue a new token',
    runnerImage: 'desk-foreman-workspace-runner:local',
    memoryLimit: '1g',
    cpuLimit: '2',
  },
  validation: {
    nameRequired: 'Name is required',
    endpointRequired: 'Endpoint is required',
  },
  notifications: {
    registered: 'Runner manager registered',
    updated: 'Runner manager updated',
  },
  errors: {
    load: 'Failed to load runner managers',
    save: 'Failed to save runner manager',
    create: 'Failed to create runner manager',
    update: 'Failed to update runner manager',
  },
  token: {
    title: 'Runner token',
    description:
      'The runner manager needs this token to authenticate with the control plane.',
    copyTitle: 'Copy this token now',
    copyDescription: 'It is shown only once and cannot be retrieved later.',
    configure: 'Configure it on the runner manager as',
  },
}

export const zhCN: RunnerManagersMessages = {
  actions: {
    refresh: '刷新运行器管理器',
    add: '添加运行器管理器',
    edit: '编辑运行器管理器',
    cancel: '取消',
    register: '注册管理器',
    save: '保存更改',
    done: '完成',
  },
  stats: {
    online: '在线',
    offline: '离线',
    disabled: '已禁用',
  },
  searchPlaceholder: '按名称或端点搜索…',
  table: {
    manager: '管理器',
    status: '状态',
    limits: '限制',
    lastSeen: '最后活动',
  },
  empty: {
    matchingTitle: '没有匹配的管理器',
    title: '暂无运行器管理器',
    searchDescription: '请尝试其他搜索条件。',
    description: '注册运行器管理器以执行工作区任务。',
  },
  limits: '{sessions} 个会话 · 超时 {timeout} · 输出 {output}',
  pagination: '第 {page} 页，共 {totalPages} 页',
  drawer: {
    createTitle: '创建运行器管理器',
    editTitle: '编辑运行器管理器',
  },
  sections: {
    connection: '连接',
    container: '容器',
    resourceLimits: '资源限制',
  },
  fields: {
    name: '名称',
    endpoint: '端点',
    hostWorkspaceRoot: '主机工作区根目录',
    existingToken: '现有令牌',
    enabled: '启用',
    runnerImage: '运行器镜像',
    networkAccess: '网络访问',
    maxSessions: '最大会话数',
    maxOutput: '最大输出（字节）',
    maxTimeout: '最大超时（毫秒）',
    pidLimit: 'PID 限制',
    memoryLimit: '内存限制',
    cpuLimit: 'CPU 限制',
  },
  hints: {
    hostWorkspaceRoot:
      '可选：运行器工作区绑定挂载的主机路径，必须与运行器管理器挂载的工作区目录一致',
    existingToken: '可选：复用之前签发的运行器令牌',
    enabled: '禁用的管理器将停止接收任务',
    networkAccess: '允许运行器容器访问出站网络',
    imageAndNetwork: '镜像和网络更改仅对新启动的运行器容器生效。',
    limits: '每个任务都会执行超时和输出限制，无论任务新旧。',
  },
  placeholders: {
    name: '例如 homelab-docker',
    endpoint: 'http://runner-manager:3001',
    hostWorkspaceRoot: '/opt/desk-foreman/vol/workspace',
    existingToken: '留空以签发新令牌',
    runnerImage: 'desk-foreman-workspace-runner:local',
    memoryLimit: '1g',
    cpuLimit: '2',
  },
  validation: {
    nameRequired: '名称为必填项',
    endpointRequired: '端点为必填项',
  },
  notifications: {
    registered: '运行器管理器已注册',
    updated: '运行器管理器已更新',
  },
  errors: {
    load: '加载运行器管理器失败',
    save: '保存运行器管理器失败',
    create: '创建运行器管理器失败',
    update: '更新运行器管理器失败',
  },
  token: {
    title: '运行器令牌',
    description: '运行器管理器需要此令牌来向控制平面进行身份验证。',
    copyTitle: '请立即复制此令牌',
    copyDescription: '令牌只显示一次，之后无法再次获取。',
    configure: '在运行器管理器上将其配置为',
  },
}

export default runnerManagers
