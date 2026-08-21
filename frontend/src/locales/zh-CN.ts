import type { MessageSchema } from './schema'
import { zhCN as approval } from './approval'
import { zhCN as audit } from './audit'
import { zhCN as auth } from './auth'
import { zhCN as dashboard } from './dashboard'

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
  shared: {
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
    },
    notifications: {
      copied: '已复制',
      copyFailed: '复制失败',
    },
  },
} satisfies MessageSchema

export default zhCN
