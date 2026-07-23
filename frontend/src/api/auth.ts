import { computed, ref } from 'vue'
import {
  client,
  login,
  logout,
  me,
  type AuthLoginRequest,
  type AuthMeResponse,
} from './generated'
import { requireOk } from './http'

const currentUser = ref<AuthMeResponse['user'] | null>(null)
const initializing = ref(false)
let initialized = false

client.setConfig({
  baseUrl: window.location.origin,
  credentials: 'include',
})

async function initialize(force = false): Promise<void> {
  if ((initialized && !force) || initializing.value) return
  initializing.value = true
  try {
    const { data, response } = await me()
    if (response?.ok && data?.user) {
      currentUser.value = data.user
    } else {
      currentUser.value = null
    }
  } finally {
    initialized = true
    initializing.value = false
  }
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
}

export const authState = {
  currentUser,
  initializing: computed(() => initializing.value),
  initialize,
  loginWithPassword,
  logoutCurrentUser,
}
