export interface AuditMessages {
  errors: {
    load: string
  }
  refresh: string
  filterPlaceholder: string
  outcomes: {
    all: string
    success: string
    failure: string
    unknown: string
  }
  eventsCount: string
  columns: {
    time: string
    action: string
    actor: string
    target: string
    outcome: string
  }
  noEvents: string
  noEventsDescription: string
  detailsAriaLabel: string
  detailTitle: string
  detail: {
    time: string
    action: string
    actor: string
    target: string
    outcome: string
    duration: string
    requestId: string
    payload: string
  }
  page: string
  notAvailable: string
  status: {
    success: string
    failure: string
    unknown: string
  }
}

const audit = {
  errors: {
    load: 'Failed to load audit logs',
  },
  refresh: 'Refresh audit logs',
  filterPlaceholder: 'Filter by action, actor or target…',
  outcomes: {
    all: 'All outcomes',
    success: 'Success',
    failure: 'Failure',
    unknown: 'Unknown',
  },
  eventsCount: '{shown} of {total} events',
  columns: {
    time: 'Time',
    action: 'Action',
    actor: 'Actor',
    target: 'Target',
    outcome: 'Outcome',
  },
  noEvents: 'No audit events',
  noEventsDescription:
    'Events appear as actions are performed on the control plane.',
  detailsAriaLabel: 'Audit event details',
  detailTitle: 'Audit event',
  detail: {
    time: 'Time',
    action: 'Action',
    actor: 'Actor',
    target: 'Target',
    outcome: 'Outcome',
    duration: 'Duration',
    requestId: 'Request ID',
    payload: 'Payload',
  },
  page: 'Page {page} of {totalPages}',
  notAvailable: '—',
  status: {
    success: 'Success',
    failure: 'Failure',
    unknown: 'Unknown',
  },
}

export const zhCN: AuditMessages = {
  errors: {
    load: '加载审计日志失败',
  },
  refresh: '刷新审计日志',
  filterPlaceholder: '按操作、操作者或目标筛选…',
  outcomes: {
    all: '所有结果',
    success: '成功',
    failure: '失败',
    unknown: '未知',
  },
  eventsCount: '{shown} / {total} 个事件',
  columns: {
    time: '时间',
    action: '操作',
    actor: '操作者',
    target: '目标',
    outcome: '结果',
  },
  noEvents: '暂无审计事件',
  noEventsDescription: '在控制平面执行操作后，事件会显示在这里。',
  detailsAriaLabel: '审计事件详情',
  detailTitle: '审计事件',
  detail: {
    time: '时间',
    action: '操作',
    actor: '操作者',
    target: '目标',
    outcome: '结果',
    duration: '耗时',
    requestId: '请求 ID',
    payload: '负载',
  },
  page: '第 {page} 页，共 {totalPages} 页',
  notAvailable: '—',
  status: {
    success: '成功',
    failure: '失败',
    unknown: '未知',
  },
}

export default audit
