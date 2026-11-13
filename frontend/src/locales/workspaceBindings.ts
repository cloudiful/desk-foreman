export interface WorkspaceBindingsMessages {
  actions: {
    refresh: string
    apply: string
    archive: string
    restore: string
  }
  filters: {
    applicationId: string
    externalUserId: string
    workspaceKey: string
    allStates: string
    active: string
    archived: string
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
  }
  notifications: {
    archiveSuccess: string
    restoreSuccess: string
    binding: string
  }
  errors: {
    load: string
    archive: string
    restore: string
    loadUpstreams: string
    createUpstream: string
    updateUpstream: string
    deleteUpstream: string
    toggleUpstream: string
    saveUpstream: string
  }
  identity: {
    application: string
  }
  upstreams: {
    title: string
    hint: string
    archived: { title: string; description: string }
    actions: {
      open: string
      add: string
      edit: string
      enable: string
      disable: string
      delete: string
      create: string
      save: string
      cancel: string
    }
    table: {
      alias: string
      targetUrl: string
      enabled: string
      credential: string
    }
    state: { enabled: string; disabled: string }
    credential: { present: string; absent: string }
    empty: { title: string; description: string }
    form: { createTitle: string; editTitle: string }
    fields: {
      alias: string
      aliasHint: string
      targetUrl: string
      targetUrlHint: string
      username: string
      secret: string
      secretHint: string
      credentialHint: string
      clearCredential: string
    }
    confirmDelete: { title: string; description: string }
    notifications: {
      created: string
      updated: string
      deleted: string
      enabled: string
      disabled: string
    }
    errors: { clearConflict: string; pairRequired: string }
    validation: { aliasRequired: string; targetUrlRequired: string }
  }
}

const workspaceBindings: WorkspaceBindingsMessages = {
  actions: {
    refresh: 'Refresh workspace bindings',
    apply: 'Apply',
    archive: 'Archive',
    restore: 'Restore',
  },
  filters: {
    applicationId: 'Application ID',
    externalUserId: 'External user ID',
    workspaceKey: 'Workspace key',
    allStates: 'All states',
    active: 'Active',
    archived: 'Archived',
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
  },
  notifications: {
    archiveSuccess: 'Workspace archived',
    restoreSuccess: 'Workspace restored',
    binding: 'Binding #{id}',
  },
  errors: {
    load: 'Failed to load workspace bindings',
    archive: 'Failed to archive workspace',
    restore: 'Failed to restore workspace',
    loadUpstreams: 'Failed to load Git upstreams',
    createUpstream: 'Failed to create Git upstream',
    updateUpstream: 'Failed to update Git upstream',
    deleteUpstream: 'Failed to delete Git upstream',
    toggleUpstream: 'Failed to change Git upstream state',
    saveUpstream: 'Failed to save Git upstream',
  },
  identity: {
    application: 'App',
  },
  upstreams: {
    title: 'Git upstreams for binding #{id}',
    hint: 'Upstream metadata only. "Enabled" does not test network reachability or guarantee a push.',
    archived: {
      title: 'Archived binding',
      description:
        'Archived bindings keep their Git upstreams for reference and cannot be changed.',
    },
    actions: {
      open: 'Manage Git upstreams',
      add: 'Add upstream',
      edit: 'Edit upstream',
      enable: 'Enable upstream',
      disable: 'Disable upstream',
      delete: 'Delete upstream',
      create: 'Add upstream',
      save: 'Save upstream',
      cancel: 'Cancel',
    },
    table: {
      alias: 'Alias',
      targetUrl: 'Target URL',
      enabled: 'Metadata state',
      credential: 'Credential',
    },
    state: {
      enabled: 'Enabled',
      disabled: 'Disabled',
    },
    credential: {
      present: 'Stored',
      absent: 'None',
    },
    empty: {
      title: 'No Git upstreams',
      description: 'Add an upstream alias to let this workspace resolve it.',
    },
    form: {
      createTitle: 'New upstream',
      editTitle: 'Edit upstream',
    },
    fields: {
      alias: 'Remote alias',
      aliasHint: 'origin',
      targetUrl: 'Target URL',
      targetUrlHint: 'https://git.example.com/team/repo.git',
      username: 'Credential username',
      secret: 'Credential secret',
      secretHint: 'Leave blank to keep the stored credential',
      credentialHint:
        'Write-only. Provide username and secret together to replace; leave both blank to keep.',
      clearCredential: 'Remove the stored credential',
    },
    confirmDelete: {
      title: 'Delete Git upstream',
      description:
        'Delete {alias}? The stored credential is removed and the action cannot be undone.',
    },
    notifications: {
      created: 'Git upstream created',
      updated: 'Git upstream updated',
      deleted: 'Git upstream deleted',
      enabled: 'Git upstream enabled',
      disabled: 'Git upstream disabled',
    },
    errors: {
      clearConflict:
        'Remove the credential cannot be combined with a replacement username or secret',
      pairRequired: 'Credential username and secret must be provided together',
    },
    validation: {
      aliasRequired: 'Remote alias is required',
      targetUrlRequired: 'Target URL is required',
    },
  },
}

export const zhCN: WorkspaceBindingsMessages = {
  actions: {
    refresh: '刷新工作区绑定',
    apply: '应用',
    archive: '归档',
    restore: '恢复',
  },
  filters: {
    applicationId: '应用 ID',
    externalUserId: '外部用户 ID',
    workspaceKey: '工作区密钥',
    allStates: '所有状态',
    active: '活跃',
    archived: '已归档',
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
  },
  notifications: {
    archiveSuccess: '工作区已归档',
    restoreSuccess: '工作区已恢复',
    binding: '绑定 #{id}',
  },
  errors: {
    load: '加载工作区绑定失败',
    archive: '归档工作区失败',
    restore: '恢复工作区失败',
    loadUpstreams: '加载 Git 上游失败',
    createUpstream: '创建 Git 上游失败',
    updateUpstream: '更新 Git 上游失败',
    deleteUpstream: '删除 Git 上游失败',
    toggleUpstream: '切换 Git 上游状态失败',
    saveUpstream: '保存 Git 上游失败',
  },
  identity: {
    application: '应用',
  },
  upstreams: {
    title: '绑定 #{id} 的 Git 上游',
    hint: '这里只管理元数据。“已启用”不会测试网络可达性，也不保证推送成功。',
    archived: {
      title: '已归档绑定',
      description: '已归档绑定的 Git 上游只保留供查看，不能修改。',
    },
    actions: {
      open: '管理 Git 上游',
      add: '添加上游',
      edit: '编辑上游',
      enable: '启用上游',
      disable: '禁用上游',
      delete: '删除上游',
      create: '添加上游',
      save: '保存上游',
      cancel: '取消',
    },
    table: {
      alias: '别名',
      targetUrl: '目标 URL',
      enabled: '元数据状态',
      credential: '凭据',
    },
    state: {
      enabled: '已启用',
      disabled: '已禁用',
    },
    credential: {
      present: '已保存',
      absent: '无',
    },
    empty: {
      title: '暂无 Git 上游',
      description: '添加上游别名后，该工作区即可解析它。',
    },
    form: {
      createTitle: '新建上游',
      editTitle: '编辑上游',
    },
    fields: {
      alias: '远端别名',
      aliasHint: 'origin',
      targetUrl: '目标 URL',
      targetUrlHint: 'https://git.example.com/team/repo.git',
      username: '凭据用户名',
      secret: '凭据密钥',
      secretHint: '留空则保留已保存的凭据',
      credentialHint: '只写：同时填写用户名和密钥才会替换；两者留空则保留。',
      clearCredential: '移除已保存的凭据',
    },
    confirmDelete: {
      title: '删除 Git 上游',
      description: '确定删除 {alias}？已保存的凭据会一并移除，且无法撤销。',
    },
    notifications: {
      created: 'Git 上游已创建',
      updated: 'Git 上游已更新',
      deleted: 'Git 上游已删除',
      enabled: 'Git 上游已启用',
      disabled: 'Git 上游已禁用',
    },
    errors: {
      clearConflict: '移除凭据不能与替换用户名或密钥同时进行',
      pairRequired: '凭据用户名和密钥必须同时提供',
    },
    validation: {
      aliasRequired: '远端别名为必填项',
      targetUrlRequired: '目标 URL 为必填项',
    },
  },
}

export default workspaceBindings
