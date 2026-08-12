<script setup lang="ts">
import Button from 'primevue/button'
import { computed } from 'vue'
import { RouterLink, RouterView, useRoute, useRouter } from 'vue-router'
import { authState } from '../api/auth'

const route = useRoute()
const router = useRouter()
const user = computed(() => authState.currentUser.value)
const items = computed(() => [
  { path: '/', label: 'Overview' },
  ...(user.value?.is_admin
    ? [
        { path: '/admin/users', label: 'Users' },
        { path: '/admin/applications', label: 'Applications' },
        { path: '/admin/workspace-bindings', label: 'Bindings' },
        { path: '/admin/audit', label: 'Audit' },
        { path: '/admin/operations', label: 'Operations' },
        { path: '/admin/approval', label: 'Approval' },
        { path: '/admin/runner-managers', label: 'Runner managers' },
      ]
    : []),
])

async function logout(): Promise<void> {
  await authState.logoutCurrentUser()
  await router.push('/login')
}
</script>

<template>
  <div class="min-h-screen p-4 md:p-6">
    <div class="mx-auto flex max-w-7xl gap-4 lg:gap-6">
      <aside class="app-shell-panel w-24 shrink-0 rounded-[2rem] p-3 md:w-64">
        <div class="mb-8 border-b border-black/8 pb-4">
          <div class="text-xs uppercase tracking-[0.35em] text-[var(--muted)]">
            Desk Foreman
          </div>
          <div class="mt-2 text-2xl font-semibold">Admin</div>
        </div>
        <nav class="space-y-2">
          <RouterLink
            v-for="item in items"
            :key="item.path"
            :to="item.path"
            class="block rounded-2xl px-4 py-3 text-sm no-underline transition"
            :class="
              route.path === item.path || route.path.startsWith(`${item.path}/`)
                ? 'bg-[var(--accent)] text-white'
                : 'bg-white/35 text-[var(--ink)] hover:bg-white/60'
            "
          >
            {{ item.label }}
          </RouterLink>
        </nav>
      </aside>

      <main class="min-w-0 flex-1">
        <header class="app-shell-panel mb-4 rounded-[2rem] px-5 py-4">
          <div
            class="flex flex-col gap-3 md:flex-row md:items-center md:justify-between"
          >
            <div>
              <div
                class="text-xs uppercase tracking-[0.25em] text-[var(--muted)]"
              >
                Workspace
              </div>
              <div class="mt-1 text-2xl font-semibold">
                Multi-user control plane
              </div>
            </div>
            <div class="flex items-center gap-3">
              <div class="text-right">
                <div class="text-sm font-medium">{{ user?.display_name }}</div>
                <div class="text-xs text-[var(--muted)]">{{ user?.email }}</div>
              </div>
              <Button label="Logout" severity="secondary" @click="logout" />
            </div>
          </div>
        </header>

        <RouterView />
      </main>
    </div>
  </div>
</template>
