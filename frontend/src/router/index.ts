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
          path: 'admin/approval',
          component: () => import('../pages/ApprovalSettingsPage.vue'),
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
  if (to.meta.requiresAuth && !authState.currentUser.value) {
    return '/login'
  }
  if (to.meta.requiresAdmin && !authState.currentUser.value?.is_admin) {
    return '/'
  }
  return true
})

export default router
