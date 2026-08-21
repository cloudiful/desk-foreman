import type { MessageSchema } from './schema'
import { zhCN as applications } from './applications'
import { zhCN as approval } from './approval'
import { zhCN as audit } from './audit'
import { zhCN as auth } from './auth'
import { zhCN as dashboard } from './dashboard'
import { zhCN as operations } from './operations'
import { zhCN as runnerManagers } from './runnerManagers'
import { zhCN as users } from './users'
import { zhCN as workspaceBindings } from './workspaceBindings'

const zhCN = {
  app: {
    errorBoundary: {
      title: '页面无法呈现',
      description: '请重新加载页面以恢复应用。',
      reload: '重新加载',
    },
  },
  shell: {
    productName: 'Desk Foreman',
    subtitle: '控制平面',
    navigation: {
      overview: '概览',
      users: '用户',
      applications: '应用',
      runnerManagers: '运行器管理器',
      operations: '操作',
      bindings: '绑定',
      audit: '审计日志',
      approval: '审批',
    },
    changePassword: '修改密码',
    signOut: '退出登录',
    openNavigation: '打开导航',
    openUserMenu: '打开用户菜单',
    switchToLightMode: '切换到浅色模式',
    switchToDarkMode: '切换到深色模式',
    language: '语言',
    languages: {
      enUS: '英语',
      zhCN: '简体中文',
    },
    user: '用户',
  },
  auth,
  dashboard,
  audit,
  approval,
  users,
  applications,
  runnerManagers,
  operations,
  workspaceBindings,
  shared: {
    requestFailed: '请求失败',
    duration: {
      seconds: '{value}秒',
      minutes: '{value}分钟',
      hours: '{value}小时',
    },
    confirm: {
      cancel: '取消',
      confirm: '确认',
    },
    dataTable: {
      noData: '暂无数据',
      nothingToShow: '暂无内容。',
      loading: '加载中…',
    },
    empty: {
      noData: '暂无数据',
    },
    errorAlert: {
      description: '请求失败，请检查详情后重试。',
      retry: '重试',
    },
    token: {
      copyAriaLabel: '复制令牌',
      copied: '令牌已复制',
      create: '创建令牌',
      revoke: '撤销',
      createTitle: '创建令牌',
      copyTitle: '请立即复制此令牌',
      copyDescription: '完整令牌只显示一次，之后无法再次获取。',
      active: '活跃令牌',
      created: '创建于 {time}',
      expires: '过期于 {time}',
      noScopes: '无权限范围',
      pagination: '第 {page} 页，共 {totalPages} 页',
      tokenName: '令牌名称',
      scopes: '权限范围',
      expiresAt: '过期时间',
      optional: '可选',
      revokeAriaLabel: '撤销 {tokenName}',
      createdNotification: '令牌已创建',
      revokedNotification: '令牌已撤销',
    },
    notifications: {
      copied: '已复制',
      copyFailed: '复制失败',
    },
    status: {
      online: '在线',
      offline: '离线',
      active: '活跃',
      inactive: '未启用',
      disabled: '已禁用',
      success: '成功',
      failure: '失败',
      failed: '失败',
      error: '错误',
      running: '运行中',
      pending: '等待中',
      idle: '空闲',
      stale: '已过期',
      archived: '已归档',
      cleanup_failed: '清理失败',
      removed: '已移除',
      cancelled: '已取消',
      timed_out: '已超时',
      exited: '已退出',
      resetting: '重置中',
      ready: '就绪',
      unknown: '未知',
    },
  },
} satisfies MessageSchema

export default zhCN
