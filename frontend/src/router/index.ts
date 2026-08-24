import { createRouter, createWebHistory } from 'vue-router'
import AppShell from '../layouts/AppShell.vue'
import { authState } from '../api/auth'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: '/login',
      component: () => import('../pages/LoginPage.vue'),
      meta: { guestOnly: true },
    },
    {
      path: '/change-password',
      component: () => import('../pages/ChangePasswordPage.vue'),
      meta: { requiresAuth: true, passwordChange: true },
    },
    {
      path: '/',
      component: AppShell,
      meta: { requiresAuth: true },
      children: [
        { path: '', component: () => import('../pages/DashboardPage.vue') },
        {
          path: 'admin/users',
          component: () => import('../pages/UsersPage.vue'),
          meta: { requiresAdmin: true },
        },
        {
          path: 'admin/applications',
          component: () => import('../pages/ApplicationsPage.vue'),
          meta: { requiresAdmin: true },
        },
        {
          path: 'admin/workspace-bindings',
          component: () => import('../pages/WorkspaceBindingsPage.vue'),
          meta: { requiresAdmin: true },
        },
        {
          path: 'admin/audit',
          component: () => import('../pages/AuditPage.vue'),
          meta: { requiresAdmin: true },
        },
        {
          path: 'admin/operations',
          component: () => import('../pages/OperationsPage.vue'),
          meta: { requiresAdmin: true },
        },
        {
          path: 'admin/runner-managers',
          component: () => import('../pages/RunnerManagersPage.vue'),
          meta: { requiresAdmin: true },
        },
      ],
    },
  ],
})

router.beforeEach(async (to) => {
  await authState.initialize()
  if (to.meta.guestOnly && authState.currentUser.value) {
    return '/'
  }
  if (
    authState.currentUser.value?.must_change_password &&
    !to.meta.passwordChange
  ) {
    return '/change-password'
  }
  if (
    !authState.currentUser.value?.must_change_password &&
    to.meta.passwordChange
  ) {
    return '/'
  }
  if (to.meta.requiresAuth && !authState.currentUser.value) {
    return '/login'
  }
  if (to.meta.requiresAdmin && !authState.currentUser.value?.is_admin) {
    return '/'
  }
  return true
})

const chunkErrorPattern =
  /Failed to fetch dynamically imported module|Importing a module script failed|ChunkLoadError/i
const chunkReloadKey = 'desk-foreman-chunk-reload'

router.onError((error, to) => {
  if (!chunkErrorPattern.test(String(error))) return
  try {
    if (sessionStorage.getItem(chunkReloadKey) === to.fullPath) return
    sessionStorage.setItem(chunkReloadKey, to.fullPath)
  } catch {
    // Continue with a best-effort reload when session storage is unavailable.
  }
  window.location.assign(to.fullPath)
})

router.afterEach(() => {
  try {
    sessionStorage.removeItem(chunkReloadKey)
  } catch {
    // Ignore unavailable session storage.
  }
})

export default router
