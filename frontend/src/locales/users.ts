export interface UsersMessages {
  actions: {
    refresh: string
    create: string
    edit: string
    tokens: string
    more: string
    resetPassword: string
    deactivate: string
    activate: string
    saveChanges: string
  }
  searchPlaceholder: string
  filters: {
    allRoles: string
    admins: string
    users: string
    allStatuses: string
    active: string
    inactive: string
  }
  count: string
  table: {
    user: string
    role: string
    status: string
    lastLogin: string
    updated: string
  }
  empty: {
    matchingTitle: string
    title: string
    searchDescription: string
    description: string
  }
  roles: {
    admin: string
    user: string
  }
  pagination: string
  drawer: {
    createTitle: string
    editTitle: string
  }
  fields: {
    loginName: string
    password: string
    displayName: string
    email: string
    timezone: string
    workspaceRoot: string
    administrator: string
    active: string
  }
  hints: {
    password: string
    workspaceRoot: string
    administrator: string
    active: string
  }
  placeholders: {
    loginName: string
    password: string
    displayName: string
    email: string
    newPassword: string
    tokenName: string
  }
  validation: {
    loginNameRequired: string
    displayNameRequired: string
    emailRequired: string
    passwordRequired: string
    passwordLength: string
  }
  notifications: {
    userCreated: string
    userCreatedDescription: string
    userUpdated: string
    userDeactivated: string
    userActivated: string
    passwordReset: string
  }
  errors: {
    load: string
    save: string
    deactivate: string
    activate: string
    reset: string
    loadTokens: string
    createToken: string
    revokeToken: string
  }
  confirmations: {
    resetTitle: string
    resetDescription: string
    deactivateTitle: string
    deactivateDescription: string
    revokeTitle: string
    revokeDescription: string
  }
  tokens: {
    title: string
    noTokens: string
  }
}

const users: UsersMessages = {
  actions: {
    refresh: 'Refresh',
    create: 'Create user',
    edit: 'Edit user',
    tokens: 'MCP tokens',
    more: 'More actions',
    resetPassword: 'Reset password',
    deactivate: 'Deactivate',
    activate: 'Activate',
    saveChanges: 'Save changes',
  },
  searchPlaceholder: 'Search by name, login or email…',
  filters: {
    allRoles: 'All roles',
    admins: 'Admins',
    users: 'Users',
    allStatuses: 'All statuses',
    active: 'Active',
    inactive: 'Inactive',
  },
  count: '{shown} of {total} users',
  table: {
    user: 'User',
    role: 'Role',
    status: 'Status',
    lastLogin: 'Last login',
    updated: 'Updated',
  },
  empty: {
    matchingTitle: 'No matching users',
    title: 'No users yet',
    searchDescription: 'Try a different search or filter.',
    description: 'Create the first user to get started.',
  },
  roles: {
    admin: 'Admin',
    user: 'User',
  },
  pagination: 'Page {page} of {totalPages}',
  drawer: {
    createTitle: 'Create user',
    editTitle: 'Edit user',
  },
  fields: {
    loginName: 'Login name',
    password: 'Password',
    displayName: 'Display name',
    email: 'Email',
    timezone: 'Timezone',
    workspaceRoot: 'Workspace root',
    administrator: 'Administrator',
    active: 'Active',
  },
  hints: {
    password: 'At least 8 characters',
    workspaceRoot: 'Assigned by the server; changeable only via database',
    administrator: 'Full access to the control plane',
    active: 'Inactive users cannot sign in',
  },
  placeholders: {
    loginName: 'e.g. alice',
    password: '••••••••',
    displayName: 'e.g. Alice',
    email: 'alice@example.com',
    newPassword: 'New password',
    tokenName: 'e.g. my-mcp-client',
  },
  validation: {
    loginNameRequired: 'Login name is required',
    displayNameRequired: 'Display name is required',
    emailRequired: 'Email is required',
    passwordRequired: 'Password is required',
    passwordLength: 'Password must be at least 8 characters',
  },
  notifications: {
    userCreated: 'User created',
    userCreatedDescription: '{login} can now sign in',
    userUpdated: 'User updated',
    userDeactivated: 'User deactivated',
    userActivated: 'User activated',
    passwordReset: 'Password reset',
  },
  errors: {
    load: 'Failed to load users',
    save: 'Failed to save user',
    deactivate: 'Failed to deactivate user',
    activate: 'Failed to activate user',
    reset: 'Failed to reset password',
    loadTokens: 'Failed to load MCP tokens',
    createToken: 'Failed to create token',
    revokeToken: 'Failed to revoke token',
  },
  confirmations: {
    resetTitle: 'Reset password',
    resetDescription: 'Set a new password for {displayName} ({login}).',
    deactivateTitle: 'Deactivate user',
    deactivateDescription:
      '{displayName} will no longer be able to sign in. Sessions are revoked.',
    revokeTitle: 'Revoke token',
    revokeDescription:
      'Clients using {tokenName} will lose access immediately.',
  },
  tokens: {
    title: 'MCP tokens',
    noTokens: 'No tokens for this user yet.',
  },
}

export const zhCN: UsersMessages = {
  actions: {
    refresh: '刷新',
    create: '创建用户',
    edit: '编辑用户',
    tokens: 'MCP 令牌',
    more: '更多操作',
    resetPassword: '重置密码',
    deactivate: '停用',
    activate: '启用',
    saveChanges: '保存更改',
  },
  searchPlaceholder: '按名称、登录名或邮箱搜索…',
  filters: {
    allRoles: '所有角色',
    admins: '管理员',
    users: '用户',
    allStatuses: '所有状态',
    active: '活跃',
    inactive: '未启用',
  },
  count: '当前显示 {shown} 个，共 {total} 个用户',
  table: {
    user: '用户',
    role: '角色',
    status: '状态',
    lastLogin: '最后登录',
    updated: '更新时间',
  },
  empty: {
    matchingTitle: '没有匹配的用户',
    title: '暂无用户',
    searchDescription: '请尝试其他搜索条件或筛选器。',
    description: '创建第一个用户以开始使用。',
  },
  roles: {
    admin: '管理员',
    user: '用户',
  },
  pagination: '第 {page} 页，共 {totalPages} 页',
  drawer: {
    createTitle: '创建用户',
    editTitle: '编辑用户',
  },
  fields: {
    loginName: '登录名',
    password: '密码',
    displayName: '显示名称',
    email: '邮箱',
    timezone: '时区',
    workspaceRoot: '工作区根目录',
    administrator: '管理员',
    active: '启用',
  },
  hints: {
    password: '至少 8 个字符',
    workspaceRoot: '由服务器分配，只能通过数据库修改',
    administrator: '拥有控制平面的全部访问权限',
    active: '未启用的用户无法登录',
  },
  placeholders: {
    loginName: '例如 alice',
    password: '••••••••',
    displayName: '例如 Alice',
    email: 'alice@example.com',
    newPassword: '新密码',
    tokenName: '例如 my-mcp-client',
  },
  validation: {
    loginNameRequired: '登录名为必填项',
    displayNameRequired: '显示名称为必填项',
    emailRequired: '邮箱为必填项',
    passwordRequired: '密码为必填项',
    passwordLength: '密码至少需要 8 个字符',
  },
  notifications: {
    userCreated: '用户已创建',
    userCreatedDescription: '{login} 现在可以登录',
    userUpdated: '用户已更新',
    userDeactivated: '用户已停用',
    userActivated: '用户已启用',
    passwordReset: '密码已重置',
  },
  errors: {
    load: '加载用户失败',
    save: '保存用户失败',
    deactivate: '停用用户失败',
    activate: '启用用户失败',
    reset: '重置密码失败',
    loadTokens: '加载 MCP 令牌失败',
    createToken: '创建令牌失败',
    revokeToken: '撤销令牌失败',
  },
  confirmations: {
    resetTitle: '重置密码',
    resetDescription: '为 {displayName}（{login}）设置新密码。',
    deactivateTitle: '停用用户',
    deactivateDescription: '{displayName} 将无法再登录，现有会话也会被撤销。',
    revokeTitle: '撤销令牌',
    revokeDescription: '使用 {tokenName} 的客户端将立即失去访问权限。',
  },
  tokens: {
    title: 'MCP 令牌',
    noTokens: '此用户暂无令牌。',
  },
}

export default users
