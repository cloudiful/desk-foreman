<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import {
  getAdminOperationsSummary,
  listAdminRunnerManagers,
} from '../api/users'
import { useAsyncData } from '../composables/useAsyncData'
import { formatRelative } from '../utils/format'
import type { RunnerManagerResponse } from '../generated/openapi/types.gen'

const router = useRouter()

const summary = useAsyncData(() => getAdminOperationsSummary())
const managers = useAsyncData(() => listAdminRunnerManagers())

const managerStats = computed(() => {
  const all = managers.data.value ?? []
  return {
    total: all.length,
    online: all.filter((m) => m.status === 'online').length,
    offline: all.filter((m) => m.status === 'offline').length,
    disabled: all.filter((m) => !m.enabled).length,
  }
})

const statCards = computed(() => [
  {
    title: 'Active runners',
    value: summary.data.value?.active_runners ?? 0,
    icon: 'i-lucide-play-circle',
  },
  {
    title: 'Active sessions',
    value: summary.data.value?.active_sessions ?? 0,
    icon: 'i-lucide-terminal-square',
  },
  {
    title: 'Failed operations',
    value: summary.data.value?.failed_operations ?? 0,
    icon: 'i-lucide-triangle-alert',
  },
  {
    title: 'Archived workspaces',
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
    <PageHeader title="Overview" description="Control plane status at a glance">
      <template #actions>
        <UButton
          icon="i-lucide-refresh-cw"
          variant="outline"
          color="neutral"
          :loading="summary.loading.value || managers.loading.value"
          @click="refresh"
        >
          Refresh
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
              Runner managers
            </h2>
            <p class="text-xs text-(--ui-text-muted)">
              {{ managerStats.online }} online ·
              {{ managerStats.offline }} offline ·
              {{ managerStats.disabled }} disabled
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
            Manage
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
          v-else-if="(managers.data.value ?? []).length"
          class="divide-y divide-(--ui-border-muted)"
        >
          <li
            v-for="manager in managers.data.value ?? []"
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
              <div>Last seen {{ formatRelative(manager.last_seen_at) }}</div>
            </div>
          </li>
        </ul>
        <EmptyState
          v-else
          title="No runner managers"
          description="Register a runner manager to start executing workspace jobs."
          class="py-10"
        />
      </section>

      <section
        class="rounded-xl border border-(--ui-border) bg-(--ui-bg) p-5 shadow-sm lg:col-span-2"
      >
        <h2 class="text-sm font-semibold text-(--ui-text-highlighted)">
          Quick actions
        </h2>
        <p class="mt-1 text-xs text-(--ui-text-muted)">
          Common administration tasks
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
            Create a user
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
            Create an application
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
            Register a runner manager
          </UButton>
        </div>
      </section>
    </div>
  </div>
</template>
