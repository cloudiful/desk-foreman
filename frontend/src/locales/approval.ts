export interface ApprovalMessages {
  refresh: string
  testReviewer: string
  saveSettings: string
  configured: string
  yes: string
  no: string
  updated: string
  apiKey: string
  missing: string
  source: string
  mode: string
  enabled: string
  disabled: string
  inheritedDescription: string
  enableAutomaticReview: string
  automaticReviewDescription: string
  endpoint: string
  endpointHint: string
  endpointPlaceholder: string
  model: string
  modelPlaceholder: string
  apiKeyHint: string
  apiKeyPlaceholder: string
  clearStoredApiKey: string
  timeout: string
  maxInput: string
  concurrentReviews: string
  maxOutputTokens: string
  testPassed: string
  testFailed: string
  testDescription: string
  apiKeySources: {
    database: string
    environment: string
    none: string
  }
  notifications: {
    saved: string
    saveFailed: string
  }
  errors: {
    load: string
    update: string
    test: string
  }
}

const approval = {
  refresh: 'Refresh approval settings',
  testReviewer: 'Test reviewer',
  saveSettings: 'Save settings',
  configured: 'Configured',
  yes: 'Yes',
  no: 'No',
  updated: 'Updated {time}',
  apiKey: 'API key',
  missing: 'Missing',
  source: 'Source: {source}',
  mode: 'Mode',
  enabled: 'Enabled',
  disabled: 'Disabled',
  inheritedDescription: 'Applies to applications inheriting global settings',
  enableAutomaticReview: 'Enable automatic review',
  automaticReviewDescription:
    'Applications using inherit will call this reviewer when enabled',
  endpoint: 'Responses API base URL',
  endpointHint: 'Base URL of an OpenAI-compatible Responses API',
  endpointPlaceholder: 'https://api.openai.com/v1',
  model: 'Model',
  modelPlaceholder: 'Reviewer model',
  apiKeyHint: 'Leave blank to keep the stored key',
  apiKeyPlaceholder: 'Enter a new reviewer API key',
  clearStoredApiKey: 'Clear stored API key',
  timeout: 'Timeout (ms)',
  maxInput: 'Max input (bytes)',
  concurrentReviews: 'Concurrent reviews',
  maxOutputTokens: 'Max output tokens',
  testPassed: 'Reviewer test passed',
  testFailed: 'Reviewer test failed',
  testDescription: '{message} ({latency} ms)',
  apiKeySources: {
    database: 'database',
    environment: 'environment',
    none: 'none',
  },
  notifications: {
    saved: 'Approval settings saved',
    saveFailed: 'Failed to save approval settings',
  },
  errors: {
    load: 'Failed to load approval settings',
    update: 'Failed to update approval settings',
    test: 'Failed to test approval reviewer',
  },
}

export const zhCN: ApprovalMessages = {
  refresh: '刷新审批设置',
  testReviewer: '测试审查器',
  saveSettings: '保存设置',
  configured: '已配置',
  yes: '是',
  no: '否',
  updated: '更新时间：{time}',
  apiKey: 'API 密钥',
  missing: '缺失',
  source: '来源：{source}',
  mode: '模式',
  enabled: '已启用',
  disabled: '已禁用',
  inheritedDescription: '应用继承全局设置时适用',
  enableAutomaticReview: '启用自动审查',
  automaticReviewDescription: '使用 inherit 的应用将在启用后调用此审查器',
  endpoint: 'Responses API 基础 URL',
  endpointHint: '兼容 OpenAI Responses API 的基础 URL',
  endpointPlaceholder: 'https://api.openai.com/v1',
  model: '模型',
  modelPlaceholder: '审查模型',
  apiKeyHint: '留空以保留已存储的密钥',
  apiKeyPlaceholder: '输入新的审查器 API 密钥',
  clearStoredApiKey: '清除已存储的 API 密钥',
  timeout: '超时（毫秒）',
  maxInput: '最大输入（字节）',
  concurrentReviews: '并发审查数',
  maxOutputTokens: '最大输出令牌数',
  testPassed: '审查器测试通过',
  testFailed: '审查器测试失败',
  testDescription: '{message}（{latency} 毫秒）',
  apiKeySources: {
    database: '数据库',
    environment: '环境变量',
    none: '无',
  },
  notifications: {
    saved: '审批设置已保存',
    saveFailed: '保存审批设置失败',
  },
  errors: {
    load: '加载审批设置失败',
    update: '更新审批设置失败',
    test: '测试审批审查器失败',
  },
}

export default approval
