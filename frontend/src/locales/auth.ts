export interface AuthMessages {
  login: {
    subtitle: string
    loginName: string
    loginNamePlaceholder: string
    password: string
    passwordPlaceholder: string
    signIn: string
    required: string
  }
  changePassword: {
    title: string
    subtitle: string
    currentPassword: string
    newPassword: string
    newPasswordHint: string
    confirmPassword: string
    changePassword: string
    currentPasswordRequired: string
    minimumLength: string
    mismatch: string
  }
  errors: {
    loginFailed: string
    logoutFailed: string
    changePasswordFailed: string
  }
}

const auth = {
  login: {
    subtitle: 'Sign in to the control plane',
    loginName: 'Login name',
    loginNamePlaceholder: 'admin',
    password: 'Password',
    passwordPlaceholder: '••••••••',
    signIn: 'Sign in',
    required: 'Enter your login name and password',
  },
  changePassword: {
    title: 'Set a new password',
    subtitle: 'You must change the default password before continuing',
    currentPassword: 'Current password',
    newPassword: 'New password',
    newPasswordHint: 'At least 8 characters',
    confirmPassword: 'Confirm new password',
    changePassword: 'Change password',
    currentPasswordRequired: 'Enter your current password',
    minimumLength: 'New password must be at least 8 characters',
    mismatch: 'Passwords do not match',
  },
  errors: {
    loginFailed: 'Login failed',
    logoutFailed: 'Logout failed',
    changePasswordFailed: 'Failed to change password',
  },
}

export const zhCN: AuthMessages = {
  login: {
    subtitle: '登录控制平面',
    loginName: '登录名',
    loginNamePlaceholder: 'admin',
    password: '密码',
    passwordPlaceholder: '••••••••',
    signIn: '登录',
    required: '请输入登录名和密码',
  },
  changePassword: {
    title: '设置新密码',
    subtitle: '继续之前必须修改默认密码',
    currentPassword: '当前密码',
    newPassword: '新密码',
    newPasswordHint: '至少 8 个字符',
    confirmPassword: '确认新密码',
    changePassword: '修改密码',
    currentPasswordRequired: '请输入当前密码',
    minimumLength: '新密码至少需要 8 个字符',
    mismatch: '两次输入的密码不一致',
  },
  errors: {
    loginFailed: '登录失败',
    logoutFailed: '退出登录失败',
    changePasswordFailed: '修改密码失败',
  },
}

export default auth
