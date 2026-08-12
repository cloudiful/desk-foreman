<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import {
  listAdminRunnerSessions,
  listAdminWorkspaceRunners,
} from '../api/users'
import { useNotify } from '../composables/useNotify'
import { formatDateTime, formatDuration } from '../utils/format'
import type {
  RunnerSessionResponse,
  WorkspaceRunnerResponse,
} from '../generated/openapi/types.gen'

const { error: notifyError } = useNotify()

const runners = ref<WorkspaceRunnerResponse[]>([])
const sessions = ref<RunnerSessionResponse[]>([])
const loading = ref(false)
const error = ref('')
const activeTab = ref('runners')
const autoRefresh = ref(true)
const runnerDetail = ref<WorkspaceRunnerResponse | null>(null)
const sessionDetail = ref<RunnerSessionResponse | null>(null)

const runnerStats = computed(() => ({
  running: runners.value.filter((r) => r.status === 'running').length,
  idle: runners.value.filter((r) => r.status === 'idle').length,
  failed: runners.value.filter((r) => r.status === 'failed').length,
  total: runners.value.length,
}))

const runnerDetailOpen = computed<boolean>({
  get: () => Boolean(runnerDetail.value),
  set: (open) => {
    if (!open) runnerDetail.value = null
  },
})

const sessionDetailOpen = computed<boolean>({
  get: () => Boolean(sessionDetail.value),
  set: (open) => {
    if (!open) sessionDetail.value = null
  },
})

const sessionStats = computed(() => {
  const active = sessions.value.filter(
    (s) => s.state === 'running' || s.state === 'pending',
  ).length
  return { active, total: sessions.value.length }
})

let timer: ReturnType<typeof setInterval> | undefined

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    const [runnerList, sessionList] = await Promise.all([
      listAdminWorkspaceRunners(),
      listAdminRunnerSessions(),
    ])
    runners.value = runnerList
    sessions.value = sessionList
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to load runner status'
    notifyError(
      'Failed to refresh operations',
      err instanceof Error ? err.message : undefined,
    )
  } finally {
    loading.value = false
  }
}

function toggleAutoRefresh(value: boolean): void {
  autoRefresh.value = value
  if (timer) {
    clearInterval(timer)
    timer = undefined
  }
  if (value) {
    timer = setInterval(() => void load(), 15000)
  }
}

onMounted(() => {
  void load()
  toggleAutoRefresh(true)
})

onUnmounted(() => {
  if (timer) clearInterval(timer)
})
</script>

<template>
  <div class="space-y-6">
    <PageHeader
      title="Operations"
      description="Workspace runners and execution sessions"
    >
      <template #actions>
        <div class="flex items-center gap-2">
          <span
            class="flex items-center gap-1.5 text-sm text-(--ui-text-muted)"
          >
            <UIcon
              name="i-lucide-radar"
              :class="
                autoRefresh ? 'text-(--ui-primary)' : 'text-(--ui-text-dimmed)'
              "
              class="size-4"
            />
            Auto-refresh
          </span>
          <USwitch
            v-model="autoRefresh"
            size="sm"
            @update:model-value="toggleAutoRefresh"
          />
          <UButton
            icon="i-lucide-refresh-cw"
            variant="outline"
            color="neutral"
            :loading="loading"
            @click="load"
          />
        </div>
      </template>
    </PageHeader>

    <div class="grid gap-4 sm:grid-cols-3">
      <StatCard
        title="Active sessions"
        :value="sessionStats.active"
        icon="i-lucide-terminal-square"
      />
      <StatCard
        title="Running runners"
        :value="runnerStats.running"
        icon="i-lucide-play-circle"
      />
      <StatCard
        title="Failed runners"
        :value="runnerStats.failed"
        icon="i-lucide-triangle-alert"
      />
    </div>

    <section
      class="rounded-xl border border-(--ui-border) bg-(--ui-bg) shadow-sm"
    >
      <UTabs
        v-model="activeTab"
        :items="[
          {
            label: `Runners (${runnerStats.total})`,
            value: 'runners',
            slot: 'runners',
          },
          {
            label: `Sessions (${sessionStats.total})`,
            value: 'sessions',
            slot: 'sessions',
          },
        ]"
      >
        <template #runners>
          <ErrorAlert v-if="error" :error="error" class="m-4" @retry="load" />
          <DataTable
            :rows="runners"
            :columns="[
              { key: 'name', label: 'Runner' },
              { key: 'owner', label: 'Owner' },
              { key: 'status', label: 'Status' },
              { key: 'last_active_at', label: 'Last active' },
              { key: 'actions', label: '', class: 'text-right' },
            ]"
            :loading="loading"
            :row-key="(row) => row.runner_id as number"
            empty-title="No runners"
            empty-description="Runners appear once a workspace command has been executed."
          >
            <template #cell-name="{ row }">
              <div class="min-w-0">
                <div
                  class="truncate font-mono text-sm text-(--ui-text-highlighted)"
                >
                  {{ row.container_name }}
                </div>
                <div class="truncate text-xs text-(--ui-text-dimmed)">
                  {{ row.image_name }}
                </div>
              </div>
            </template>
            <template #cell-owner="{ row }">
              <span class="text-sm text-(--ui-text-muted)">
                {{ row.owner_kind }}
                <template v-if="row.owner_user_id">
                  #{{ row.owner_user_id }}</template
                >
              </span>
            </template>
            <template #cell-status="{ row }">
              <StatusBadge :status="row.status as string" />
            </template>
            <template #cell-last_active_at="{ row }">
              <span class="whitespace-nowrap text-sm text-(--ui-text-muted)">
                {{ formatDateTime(row.last_active_at as string) }}
              </span>
            </template>
            <template #cell-actions="{ row }">
              <div class="flex justify-end">
                <UButton
                  icon="i-lucide-eye"
                  variant="ghost"
                  color="neutral"
                  size="sm"
                  aria-label="Runner details"
                  @click="
                    () => {
                      runnerDetail = row as unknown as WorkspaceRunnerResponse
                    }
                  "
                />
              </div>
            </template>
          </DataTable>
        </template>

        <template #sessions>
          <ErrorAlert v-if="error" :error="error" class="m-4" @retry="load" />
          <DataTable
            :rows="sessions"
            :columns="[
              { key: 'session', label: 'Session' },
              { key: 'owner', label: 'Owner' },
              { key: 'state', label: 'State' },
              { key: 'exit_code', label: 'Exit' },
              { key: 'duration', label: 'Duration' },
              { key: 'actions', label: '', class: 'text-right' },
            ]"
            :loading="loading"
            :row-key="(row) => row.session_id as number"
            empty-title="No sessions"
            empty-description="Sessions appear once a workspace command has run."
          >
            <template #cell-session="{ row }">
              <span class="font-mono text-sm text-(--ui-text-highlighted)">
                #{{ row.session_id }}
              </span>
            </template>
            <template #cell-owner="{ row }">
              <span class="text-sm text-(--ui-text-muted)">
                {{ row.owner_kind }} #{{ row.owner_id }}
              </span>
            </template>
            <template #cell-state="{ row }">
              <StatusBadge :status="row.state as string" />
            </template>
            <template #cell-exit_code="{ row }">
              <span
                class="font-mono text-sm"
                :class="
                  row.exit_code === 0
                    ? 'text-(--ui-text-success)'
                    : row.exit_code != null
                      ? 'text-(--ui-text-error)'
                      : 'text-(--ui-text-muted)'
                "
              >
                {{ row.exit_code ?? '—' }}
              </span>
            </template>
            <template #cell-duration="{ row }">
              <span class="whitespace-nowrap text-sm text-(--ui-text-muted)">
                {{ formatDuration(row.wall_time_seconds as number) }}
                <template v-if="row.timed_out">
                  <UBadge
                    size="sm"
                    color="warning"
                    variant="subtle"
                    class="ml-1"
                  >
                    timeout
                  </UBadge>
                </template>
              </span>
            </template>
            <template #cell-actions="{ row }">
              <div class="flex justify-end">
                <UButton
                  icon="i-lucide-eye"
                  variant="ghost"
                  color="neutral"
                  size="sm"
                  aria-label="Session details"
                  @click="
                    () => {
                      sessionDetail = row as unknown as RunnerSessionResponse
                    }
                  "
                />
              </div>
            </template>
          </DataTable>
        </template>
      </UTabs>
    </section>

    <!-- Runner detail drawer -->
    <UDrawer v-model:open="runnerDetailOpen" title="Runner details">
      <template #body>
        <dl v-if="runnerDetail" class="space-y-3 text-sm">
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Container</dt>
            <dd class="font-mono text-right">
              {{ runnerDetail.container_name }}
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Container ID</dt>
            <dd class="font-mono text-right">
              {{ runnerDetail.container_id ?? '—' }}
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Status</dt>
            <dd><StatusBadge :status="runnerDetail.status" /></dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Runtime</dt>
            <dd>{{ runnerDetail.runtime }}</dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Image</dt>
            <dd class="font-mono text-right">{{ runnerDetail.image_name }}</dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Workspace root</dt>
            <dd class="max-w-[55%] truncate font-mono text-right">
              {{ runnerDetail.workspace_root }}
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Last active</dt>
            <dd>{{ formatDateTime(runnerDetail.last_active_at) }}</dd>
          </div>
          <div
            v-if="runnerDetail.last_error"
            class="rounded-lg border border-(--ui-border-error) bg-(--ui-bg-elevated) p-3"
          >
            <dt
              class="mb-1 text-xs font-semibold uppercase tracking-wide text-(--ui-text-error)"
            >
              Last error
            </dt>
            <dd
              class="whitespace-pre-wrap font-mono text-xs text-(--ui-text-muted)"
            >
              {{ runnerDetail.last_error }}
            </dd>
          </div>
        </dl>
      </template>
    </UDrawer>

    <!-- Session detail drawer -->
    <UDrawer v-model:open="sessionDetailOpen" title="Session details">
      <template #body>
        <dl v-if="sessionDetail" class="space-y-3 text-sm">
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Session ID</dt>
            <dd class="font-mono">{{ sessionDetail.session_id }}</dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Session key</dt>
            <dd class="max-w-[55%] truncate font-mono text-right">
              {{ sessionDetail.session_key ?? '—' }}
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Owner</dt>
            <dd>
              {{ sessionDetail.owner_kind }} #{{ sessionDetail.owner_id }}
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">State</dt>
            <dd><StatusBadge :status="sessionDetail.state" /></dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Exit code</dt>
            <dd class="font-mono">{{ sessionDetail.exit_code ?? '—' }}</dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Wall time</dt>
            <dd>{{ formatDuration(sessionDetail.wall_time_seconds) }}</dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Timed out</dt>
            <dd>{{ sessionDetail.timed_out ? 'Yes' : 'No' }}</dd>
          </div>
        </dl>
      </template>
    </UDrawer>
  </div>
</template>
