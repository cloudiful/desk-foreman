import {
  changePassword,
  login,
  logout,
  me,
  type AuthLoginRequest,
  type AuthMeResponse,
  type ChangePasswordRequest,
  type UserResponse,
} from './generated'
import { ApiError, errorMessage, responseErrorMessage } from './http'
import { i18n } from '../i18n'

export async function fetchCurrentUser(
  signal?: AbortSignal,
): Promise<UserResponse | null> {
  const { data, response } = await me({ signal })
  if (response?.status === 401) return null
  if (!response?.ok) {
    throw new ApiError(
      await responseErrorMessage(response, i18n.t('shared.requestFailed')),
      response?.status ?? 0,
    )
  }
  return data?.user ?? null
}

export async function loginWithPassword(
  body: AuthLoginRequest,
): Promise<AuthMeResponse> {
  const { data, response } = await login({ body })
  if (!response?.ok) {
    throw new ApiError(
      await responseErrorMessage(response, i18n.t('auth.errors.loginFailed')),
      response?.status ?? 0,
    )
  }
  if (!data) {
    throw new ApiError(i18n.t('auth.errors.loginFailed'), response?.status ?? 0)
  }
  return data
}

export async function logoutCurrentUser(): Promise<void> {
  const { response } = await logout()
  if (!response?.ok) {
    throw new ApiError(
      await responseErrorMessage(response, i18n.t('auth.errors.logoutFailed')),
      response?.status ?? 0,
    )
  }
}

export async function changeCurrentPassword(
  body: ChangePasswordRequest,
): Promise<void> {
  const { error, response } = await changePassword({ body })
  if (!response?.ok) {
    throw new ApiError(
      errorMessage(error, i18n.t('auth.errors.changePasswordFailed')),
      response?.status ?? 0,
    )
  }
}
