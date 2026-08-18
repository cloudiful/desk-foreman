import { computed, ref } from 'vue'
import {
  client,
  login,
  logout,
  me,
  changePassword,
  type AuthLoginRequest,
  type AuthMeResponse,
} from './generated'
import { requireOk } from './http'

const currentUser = ref<AuthMeResponse['user'] | null>(null)
const initializing = ref(false)
let initialized = false
let initializePromise: Promise<void> | null = null

client.setConfig({
  baseUrl: window.location.origin,
  credentials: 'include',
})

async function initialize(force = false): Promise<void> {
  if (initialized && !force) return
  if (initializePromise) return initializePromise
  initializePromise = (async () => {
    initializing.value = true
    try {
      const { data, response } = await me()
      currentUser.value = response?.ok && data?.user ? data.user : null
      initialized = Boolean(response?.ok || response?.status === 401)
    } catch {
      currentUser.value = null
      initialized = false
    } finally {
      initializing.value = false
      initializePromise = null
    }
  })()
  return initializePromise
}

async function loginWithPassword(body: AuthLoginRequest): Promise<void> {
  const { data, response } = await login({ body })
  await requireOk(response, 'Login failed')
  currentUser.value = data?.user ?? null
  initialized = true
}

async function logoutCurrentUser(): Promise<void> {
  const { response } = await logout()
  await requireOk(response, 'Logout failed')
  currentUser.value = null
  initialized = false
}

function invalidate(): void {
  currentUser.value = null
  initialized = false
}

async function changeCurrentPassword(
  current_password: string,
  new_password: string,
): Promise<void> {
  const { response } = await changePassword({
    body: { current_password, new_password },
  })
  await requireOk(response, 'Failed to change password')
  await initialize(true)
}

export const authState = {
  currentUser,
  initializing: computed(() => initializing.value),
  initialize,
  loginWithPassword,
  logoutCurrentUser,
  changeCurrentPassword,
  invalidate,
}
