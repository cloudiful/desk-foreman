<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import {
  getAdminOperationsSummary,
  listAdminRunnerManagers,
} from '../api/users'
import { useAsyncData } from '../composables/useAsyncData'
import { formatRelative } from '../utils/format'
import type { RunnerManagerResponse } from '../generated/openapi/types.gen'

const router = useRouter()
const { t } = useI18n()

const summary = useAsyncData(() => getAdminOperationsSummary())
const managers = useAsyncData(() => listAdminRunnerManagers({ limit: 5 }))

const managerStats = computed(() => {
  const data = summary.data.value
  return {
    total: data?.runner_managers_total ?? 0,
    online: data?.runner_managers_online ?? 0,
    offline: data?.runner_managers_offline ?? 0,
    disabled: data?.runner_managers_disabled ?? 0,
  }
})

const statCards = computed(() => [
  {
    title: t('dashboard.stats.activeRunners'),
    value: summary.data.value?.active_runners ?? 0,
    icon: 'i-lucide-play-circle',
  },
  {
    title: t('dashboard.stats.activeSessions'),
    value: summary.data.value?.active_sessions ?? 0,
    icon: 'i-lucide-terminal-square',
  },
  {
    title: t('dashboard.stats.failedOperations'),
    value: summary.data.value?.failed_operations ?? 0,
    icon: 'i-lucide-triangle-alert',
  },
  {
    title: t('dashboard.stats.archivedWorkspaces'),
    value: summary.data.value?.archived_workspaces ?? 0,
    icon: 'i-lucide-archive',
  },
])

function refresh(): void {
  void summary.load()
  void managers.load()
}

function statusOf(
  manager: RunnerManagerResponse,
): 'online' | 'offline' | 'disabled' {
  if (!manager.enabled) return 'disabled'
  return manager.status === 'online' ? 'online' : 'offline'
}

onMounted(refresh)
</script>

<template>
  <div class="space-y-6">
    <PageHeader>
      <template #actions>
        <UButton
          icon="i-lucide-refresh-cw"
          variant="outline"
          color="neutral"
          :loading="summary.loading.value || managers.loading.value"
          :aria-label="t('dashboard.refresh')"
          @click="refresh"
        >
          {{ t('dashboard.refresh') }}
        </UButton>
      </template>
    </PageHeader>

    <ErrorAlert
      v-if="summary.error.value"
      :error="summary.error.value"
      @retry="refresh"
    />

    <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
      <StatCard
        v-for="card in statCards"
        :key="card.title"
        :title="card.title"
        :value="card.value"
        :icon="card.icon"
        :loading="summary.loading.value"
      />
    </div>

    <div class="grid gap-6 lg:grid-cols-5">
      <section
        class="rounded-xl border border-(--ui-border) bg-(--ui-bg) shadow-sm lg:col-span-3"
      >
        <div
          class="flex items-center justify-between border-b border-(--ui-border) px-5 py-4"
        >
          <div>
            <h2 class="text-sm font-semibold text-(--ui-text-highlighted)">
              {{ t('dashboard.runnerManagers') }}
            </h2>
            <p class="text-xs text-(--ui-text-muted)">
              {{
                t('dashboard.runnerManagersSummary', {
                  online: managerStats.online,
                  offline: managerStats.offline,
                  disabled: managerStats.disabled,
                })
              }}
            </p>
          </div>
          <UButton
            icon="i-lucide-plus"
            size="sm"
            @click="
              () => {
                void router.push('/admin/runner-managers')
              }
            "
          >
            {{ t('dashboard.manage') }}
          </UButton>
        </div>

        <div v-if="managers.loading.value" class="space-y-3 p-5">
          <div
            v-for="i in 3"
            :key="i"
            class="h-12 animate-pulse rounded-md bg-(--ui-bg-elevated)"
          />
        </div>
        <ErrorAlert
          v-else-if="managers.error.value"
          :error="managers.error.value"
          class="m-5"
          @retry="managers.load"
        />
        <ul
          v-else-if="(managers.data.value?.items ?? []).length"
          class="divide-y divide-(--ui-border-muted)"
        >
          <li
            v-for="manager in managers.data.value?.items ?? []"
            :key="manager.runner_manager_id"
            class="flex items-center justify-between gap-3 px-5 py-3.5"
          >
            <div class="min-w-0">
              <div class="flex items-center gap-2">
                <span
                  class="truncate text-sm font-medium text-(--ui-text-highlighted)"
                >
                  {{ manager.name }}
                </span>
                <StatusBadge :status="statusOf(manager)" />
              </div>
              <p
                class="mt-0.5 truncate font-mono text-xs text-(--ui-text-muted)"
              >
                {{ manager.endpoint }}
              </p>
            </div>
            <div class="shrink-0 text-right text-xs text-(--ui-text-dimmed)">
              <div>
                {{
                  t('dashboard.lastSeen', {
                    time: formatRelative(manager.last_seen_at),
                  })
                }}
              </div>
            </div>
          </li>
        </ul>
        <EmptyState
          v-else
          :title="t('dashboard.noRunnerManagers')"
          :description="t('dashboard.noRunnerManagersDescription')"
          class="py-10"
        />
      </section>

      <section
        class="rounded-xl border border-(--ui-border) bg-(--ui-bg) p-5 shadow-sm lg:col-span-2"
      >
        <h2 class="text-sm font-semibold text-(--ui-text-highlighted)">
          {{ t('dashboard.quickActions') }}
        </h2>
        <p class="mt-1 text-xs text-(--ui-text-muted)">
          {{ t('dashboard.commonTasks') }}
        </p>
        <div class="mt-4 space-y-2">
          <UButton
            block
            variant="soft"
            leading-icon="i-lucide-user-plus"
            @click="
              () => {
                void router.push('/admin/users')
              }
            "
          >
            {{ t('dashboard.createUser') }}
          </UButton>
          <UButton
            block
            variant="soft"
            leading-icon="i-lucide-app-window"
            @click="
              () => {
                void router.push('/admin/applications')
              }
            "
          >
            {{ t('dashboard.createApplication') }}
          </UButton>
          <UButton
            block
            variant="soft"
            leading-icon="i-lucide-server-cog"
            @click="
              () => {
                void router.push('/admin/runner-managers')
              }
            "
          >
            {{ t('dashboard.registerRunnerManager') }}
          </UButton>
        </div>
      </section>
    </div>
  </div>
</template>
