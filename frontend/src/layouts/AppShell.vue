<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'
import { authState } from '../api/auth'
import { useTheme } from '../composables/useTheme'
import { locale, setLocale, type Locale } from '../i18n'

const route = useRoute()
const router = useRouter()
const { isDark, toggle } = useTheme()
const { t } = useI18n()
const navOpen = ref(false)
const user = computed(() => authState.currentUser.value)
const selectedLocale = computed<Locale>({
  get: () => locale.value,
  set: setLocale,
})
const localeOptions = computed(() => [
  { label: t('shell.languages.enUS'), value: 'en-US' as const },
  { label: t('shell.languages.zhCN'), value: 'zh-CN' as const },
])

const nav = computed(() => [
  {
    path: '/',
    label: t('shell.navigation.overview'),
    icon: 'i-lucide-layout-dashboard',
  },
  ...(user.value?.is_admin
    ? [
        {
          path: '/admin/users',
          label: t('shell.navigation.users'),
          icon: 'i-lucide-users',
        },
        {
          path: '/admin/applications',
          label: t('shell.navigation.applications'),
          icon: 'i-lucide-app-window',
        },
        {
          path: '/admin/runner-managers',
          label: t('shell.navigation.runnerManagers'),
          icon: 'i-lucide-server-cog',
        },
        {
          path: '/admin/operations',
          label: t('shell.navigation.operations'),
          icon: 'i-lucide-terminal-square',
        },
        {
          path: '/admin/workspace-bindings',
          label: t('shell.navigation.bindings'),
          icon: 'i-lucide-folder-tree',
        },
        {
          path: '/admin/audit',
          label: t('shell.navigation.audit'),
          icon: 'i-lucide-scroll-text',
        },
        {
          path: '/admin/approval',
          label: t('shell.navigation.approval'),
          icon: 'i-lucide-shield-check',
        },
      ]
    : []),
])

function isActive(path: string): boolean {
  if (path === '/') return route.path === '/'
  return route.path === path || route.path.startsWith(`${path}/`)
}

const title = computed(
  () =>
    nav.value.find((item) => isActive(item.path))?.label ??
    t('shell.productName'),
)

async function logout(): Promise<void> {
  await authState.logoutCurrentUser()
  await router.push('/login')
}
</script>

<template>
  <div class="min-h-screen bg-(--ui-bg-muted)">
    <div class="mx-auto flex max-w-7xl">
      <aside
        class="sticky top-0 hidden h-screen w-64 shrink-0 flex-col border-r border-(--ui-border) bg-(--ui-bg) lg:flex"
      >
        <div class="flex items-center gap-2.5 px-5 py-4">
          <div
            class="flex size-8 items-center justify-center rounded-md bg-(--ui-primary) text-white"
          >
            <UIcon name="i-lucide-hammer" class="size-4.5" />
          </div>
          <div>
            <div class="text-sm font-semibold text-(--ui-text-highlighted)">
              {{ t('shell.productName') }}
            </div>
            <div class="text-xs text-(--ui-text-dimmed)">
              {{ t('shell.subtitle') }}
            </div>
          </div>
        </div>
        <nav class="mt-2 flex-1 space-y-1 overflow-y-auto px-3 pb-4">
          <RouterLink
            v-for="item in nav"
            :key="item.path"
            :to="item.path"
            class="flex items-center gap-2.5 rounded-md px-3 py-2 text-sm font-medium transition-colors"
            :class="
              isActive(item.path)
                ? 'bg-(--ui-primary)/10 text-(--ui-primary)'
                : 'text-(--ui-text-muted) hover:bg-(--ui-bg-elevated) hover:text-(--ui-text-highlighted)'
            "
          >
            <UIcon :name="item.icon" class="size-4.5" />
            {{ item.label }}
          </RouterLink>
        </nav>
        <div class="border-t border-(--ui-border) p-3">
          <RouterLink
            to="/change-password"
            class="flex items-center gap-2.5 rounded-md px-3 py-2 text-sm font-medium text-(--ui-text-muted) transition-colors hover:bg-(--ui-bg-elevated) hover:text-(--ui-text-highlighted)"
          >
            <UIcon name="i-lucide-key-round" class="size-4.5" />
            {{ t('shell.changePassword') }}
          </RouterLink>
        </div>
      </aside>

      <div class="flex min-w-0 flex-1 flex-col">
        <header
          class="sticky top-0 z-20 flex items-center gap-3 border-b border-(--ui-border) bg-(--ui-bg)/80 px-4 py-3 backdrop-blur md:px-6"
        >
          <UButton
            icon="i-lucide-menu"
            variant="ghost"
            color="neutral"
            class="lg:hidden"
            :aria-label="t('shell.openNavigation')"
            @click="
              () => {
                navOpen = true
              }
            "
          />
          <div class="min-w-0 flex-1">
            <h2
              class="truncate text-sm font-semibold text-(--ui-text-highlighted)"
            >
              {{ title }}
            </h2>
          </div>
          <UButton
            :icon="isDark ? 'i-lucide-sun' : 'i-lucide-moon'"
            variant="ghost"
            color="neutral"
            :aria-label="
              isDark
                ? t('shell.switchToLightMode')
                : t('shell.switchToDarkMode')
            "
            @click="toggle"
          />
          <USelect
            v-model="selectedLocale"
            :items="localeOptions"
            leading-icon="i-lucide-languages"
            size="sm"
            class="w-32"
            :aria-label="t('shell.language')"
          />
          <UDropdownMenu
            :items="[
              [
                {
                  label: user?.display_name ?? user?.login_name ?? '',
                  type: 'label',
                },
                {
                  label: user?.email ?? '',
                  type: 'label',
                },
                { type: 'separator' },
                {
                  label: t('shell.changePassword'),
                  icon: 'i-lucide-key-round',
                  onSelect: () => router.push('/change-password'),
                },
                {
                  label: t('shell.signOut'),
                  icon: 'i-lucide-log-out',
                  color: 'error',
                  onSelect: logout,
                },
              ],
            ]"
          >
            <button
              class="flex items-center gap-2 rounded-full focus:outline-none"
              type="button"
              :aria-label="t('shell.openUserMenu')"
            >
              <UAvatar
                :alt="user?.display_name ?? user?.login_name ?? t('shell.user')"
                size="sm"
              />
            </button>
          </UDropdownMenu>
        </header>

        <main class="flex-1 px-4 py-6 md:px-6">
          <RouterView />
        </main>
      </div>
    </div>

    <USlideover
      v-model:open="navOpen"
      side="left"
      :title="t('shell.productName')"
    >
      <template #body>
        <nav class="mt-2 space-y-1 px-2">
          <RouterLink
            v-for="item in nav"
            :key="item.path"
            :to="item.path"
            class="flex items-center gap-2.5 rounded-md px-3 py-2 text-sm font-medium"
            :class="
              isActive(item.path)
                ? 'bg-(--ui-primary)/10 text-(--ui-primary)'
                : 'text-(--ui-text-muted) hover:bg-(--ui-bg-elevated)'
            "
            @click="navOpen = false"
          >
            <UIcon :name="item.icon" class="size-4.5" />
            {{ item.label }}
          </RouterLink>
        </nav>
      </template>
    </USlideover>
  </div>
</template>
