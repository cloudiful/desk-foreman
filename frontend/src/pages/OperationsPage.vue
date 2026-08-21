<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  getAdminOperationsSummary,
  listAdminRunnerSessions,
  listAdminWorkspaceRunners,
} from '../api/users'
import { useNotify } from '../composables/useNotify'
import { formatDateTime, formatDuration } from '../utils/format'
import { PAGE_SIZE, pageCount, pageOffset } from '../utils/pagination'
import type {
  ListRunnerSessionsData,
  ListWorkspaceRunnersData,
  OperationsSummary,
  RunnerSessionResponse,
  WorkspaceRunnerResponse,
} from '../generated/openapi/types.gen'

const { error: notifyError } = useNotify()
const { t } = useI18n()

const runners = ref<WorkspaceRunnerResponse[]>([])
const runnerTotal = ref(0)
const sessions = ref<RunnerSessionResponse[]>([])
const sessionTotal = ref(0)
const summary = ref<OperationsSummary | null>(null)
const loading = ref(false)
const error = ref('')
const activeTab = ref<'runners' | 'sessions'>('runners')
const runnersPage = ref(1)
const sessionsPage = ref(1)
const AUTO_REFRESH_KEY = 'desk-foreman-operations-auto-refresh'

function readAutoRefresh(): boolean {
  try {
    return window.localStorage.getItem(AUTO_REFRESH_KEY) !== 'false'
  } catch {
    return true
  }
}

const autoRefresh = ref(readAutoRefresh())
const runnerDetail = ref<WorkspaceRunnerResponse | null>(null)
const sessionDetail = ref<RunnerSessionResponse | null>(null)

const runnersTotalPages = computed(() =>
  pageCount(runnerTotal.value, PAGE_SIZE),
)
const sessionsTotalPages = computed(() =>
  pageCount(sessionTotal.value, PAGE_SIZE),
)

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

let timer: ReturnType<typeof setInterval> | undefined

async function load(manual = true): Promise<void> {
  if (loading.value) return
  loading.value = true
  error.value = ''
  try {
    const runnerQuery: NonNullable<ListWorkspaceRunnersData['query']> = {
      limit: PAGE_SIZE,
      offset: pageOffset(runnersPage.value, PAGE_SIZE),
    }
    const sessionQuery: NonNullable<ListRunnerSessionsData['query']> = {
      limit: PAGE_SIZE,
      offset: pageOffset(sessionsPage.value, PAGE_SIZE),
    }
    const [runnerPage, sessionPage, summaryData] = await Promise.all([
      listAdminWorkspaceRunners(runnerQuery),
      listAdminRunnerSessions(sessionQuery),
      getAdminOperationsSummary(),
    ])
    runners.value = runnerPage.items
    runnerTotal.value = runnerPage.total
    sessions.value = sessionPage.items
    sessionTotal.value = sessionPage.total
    summary.value = summaryData
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : t('operations.errors.load')
    if (manual) {
      notifyError(
        t('operations.errors.refresh'),
        err instanceof Error ? err.message : undefined,
      )
    }
  } finally {
    loading.value = false
  }
}

function onRunnersPageChange(): void {
  void load(true)
}

function onSessionsPageChange(): void {
  void load(true)
}

function stopAutoRefresh(): void {
  if (timer) {
    clearInterval(timer)
    timer = undefined
  }
}

function startAutoRefresh(): void {
  stopAutoRefresh()
  if (autoRefresh.value && !document.hidden) {
    timer = setInterval(() => void load(false), 15000)
  }
}

function toggleAutoRefresh(value: boolean): void {
  autoRefresh.value = value
  try {
    window.localStorage.setItem(AUTO_REFRESH_KEY, String(value))
  } catch {
    // Persistence is optional.
  }
  startAutoRefresh()
}

function handleVisibilityChange(): void {
  startAutoRefresh()
  if (!document.hidden && !loading.value) void load(false)
}

onMounted(() => {
  void load()
  document.addEventListener('visibilitychange', handleVisibilityChange)
  startAutoRefresh()
})

onUnmounted(() => {
  stopAutoRefresh()
  document.removeEventListener('visibilitychange', handleVisibilityChange)
})
</script>

<template>
  <div class="space-y-6">
    <PageHeader>
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
            {{ t('operations.autoRefresh') }}
          </span>
          <USwitch
            v-model="autoRefresh"
            size="sm"
            :aria-label="t('operations.autoRefresh')"
            @update:model-value="toggleAutoRefresh"
          />
          <UButton
            icon="i-lucide-refresh-cw"
            variant="outline"
            color="neutral"
            :loading="loading"
            :aria-label="t('operations.actions.refresh')"
            @click="() => load()"
          />
        </div>
      </template>
    </PageHeader>

    <div class="grid gap-4 sm:grid-cols-3">
      <StatCard
        :title="t('operations.stats.activeSessions')"
        :value="summary?.active_sessions ?? 0"
        icon="i-lucide-terminal-square"
      />
      <StatCard
        :title="t('operations.stats.runningRunners')"
        :value="summary?.active_runners ?? 0"
        icon="i-lucide-play-circle"
      />
      <StatCard
        :title="t('operations.stats.failedOperations')"
        :value="summary?.failed_operations ?? 0"
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
            label: t('operations.tabs.runners', { count: runnerTotal }),
            value: 'runners',
            slot: 'runners',
          },
          {
            label: t('operations.tabs.sessions', { count: sessionTotal }),
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
              { key: 'name', label: t('operations.runners.table.runner') },
              { key: 'owner', label: t('operations.runners.table.owner') },
              { key: 'status', label: t('operations.runners.table.status') },
              {
                key: 'last_active_at',
                label: t('operations.runners.table.lastActive'),
              },
              { key: 'actions', label: '', class: 'text-right' },
            ]"
            :loading="loading"
            :row-key="(row) => row.runner_id as number"
            :empty-title="t('operations.runners.emptyTitle')"
            :empty-description="t('operations.runners.emptyDescription')"
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
                  :aria-label="t('operations.runners.detailsAriaLabel')"
                  @click="
                    () => {
                      runnerDetail = row as unknown as WorkspaceRunnerResponse
                    }
                  "
                />
              </div>
            </template>
          </DataTable>

          <div
            v-if="runnersTotalPages > 1"
            class="flex items-center justify-between border-t border-(--ui-border) px-4 py-3"
          >
            <span class="text-sm text-(--ui-text-muted)">
              {{
                t('operations.pagination', {
                  page: runnersPage,
                  totalPages: runnersTotalPages,
                })
              }}
            </span>
            <UPagination
              v-model:page="runnersPage"
              :total="runnerTotal"
              :items-per-page="PAGE_SIZE"
              @update:page="onRunnersPageChange"
            />
          </div>
        </template>

        <template #sessions>
          <ErrorAlert v-if="error" :error="error" class="m-4" @retry="load" />
          <DataTable
            :rows="sessions"
            :columns="[
              { key: 'session', label: t('operations.sessions.table.session') },
              { key: 'owner', label: t('operations.sessions.table.owner') },
              { key: 'state', label: t('operations.sessions.table.state') },
              { key: 'exit_code', label: t('operations.sessions.table.exit') },
              {
                key: 'duration',
                label: t('operations.sessions.table.duration'),
              },
              { key: 'actions', label: '', class: 'text-right' },
            ]"
            :loading="loading"
            :row-key="(row) => row.session_id as number"
            :empty-title="t('operations.sessions.emptyTitle')"
            :empty-description="t('operations.sessions.emptyDescription')"
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
                    {{ t('operations.timeout') }}
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
                  :aria-label="t('operations.sessions.detailsAriaLabel')"
                  @click="
                    () => {
                      sessionDetail = row as unknown as RunnerSessionResponse
                    }
                  "
                />
              </div>
            </template>
          </DataTable>

          <div
            v-if="sessionsTotalPages > 1"
            class="flex items-center justify-between border-t border-(--ui-border) px-4 py-3"
          >
            <span class="text-sm text-(--ui-text-muted)">
              {{
                t('operations.pagination', {
                  page: sessionsPage,
                  totalPages: sessionsTotalPages,
                })
              }}
            </span>
            <UPagination
              v-model:page="sessionsPage"
              :total="sessionTotal"
              :items-per-page="PAGE_SIZE"
              @update:page="onSessionsPageChange"
            />
          </div>
        </template>
      </UTabs>
    </section>

    <!-- Runner detail drawer -->
    <UDrawer
      v-model:open="runnerDetailOpen"
      :title="t('operations.runners.detailsTitle')"
      :close="true"
    >
      <template #body>
        <dl v-if="runnerDetail" class="space-y-3 text-sm">
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.runners.details.container') }}
            </dt>
            <dd class="max-w-[55%] truncate font-mono text-right">
              {{ runnerDetail.container_name }}
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.runners.details.containerId') }}
            </dt>
            <dd class="max-w-[55%] break-all font-mono text-right">
              {{ runnerDetail.container_id ?? '—' }}
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.runners.details.runnerManager') }}
            </dt>
            <dd>{{ runnerDetail.runner_manager_id ?? '—' }}</dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.runners.details.status') }}
            </dt>
            <dd><StatusBadge :status="runnerDetail.status" /></dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.runners.details.runtime') }}
            </dt>
            <dd>{{ runnerDetail.runtime }}</dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.runners.details.image') }}
            </dt>
            <dd class="max-w-[55%] truncate font-mono text-right">
              {{ runnerDetail.image_name }}
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.runners.details.workspaceRoot') }}
            </dt>
            <dd class="max-w-[55%] truncate font-mono text-right">
              {{ runnerDetail.workspace_root }}
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.runners.details.lastActive') }}
            </dt>
            <dd>{{ formatDateTime(runnerDetail.last_active_at) }}</dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.runners.details.lastObserved') }}
            </dt>
            <dd>{{ formatDateTime(runnerDetail.last_observed_at) }}</dd>
          </div>
          <div
            v-if="runnerDetail.last_error"
            class="rounded-lg border border-(--ui-border-error) bg-(--ui-bg-elevated) p-3"
          >
            <dt
              class="mb-1 text-xs font-semibold uppercase tracking-wide text-(--ui-text-error)"
            >
              {{ t('operations.runners.details.lastError') }}
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
    <UDrawer
      v-model:open="sessionDetailOpen"
      :title="t('operations.sessions.detailsTitle')"
      :close="true"
    >
      <template #body>
        <dl v-if="sessionDetail" class="space-y-3 text-sm">
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.sessions.details.sessionId') }}
            </dt>
            <dd class="font-mono">{{ sessionDetail.session_id }}</dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.sessions.details.sessionKey') }}
            </dt>
            <dd class="max-w-[55%] truncate font-mono text-right">
              {{ sessionDetail.session_key ?? '—' }}
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.sessions.details.owner') }}
            </dt>
            <dd>
              {{ sessionDetail.owner_kind }} #{{ sessionDetail.owner_id }}
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.sessions.details.state') }}
            </dt>
            <dd><StatusBadge :status="sessionDetail.state" /></dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.sessions.details.exitCode') }}
            </dt>
            <dd class="font-mono">{{ sessionDetail.exit_code ?? '—' }}</dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.sessions.details.wallTime') }}
            </dt>
            <dd>{{ formatDuration(sessionDetail.wall_time_seconds) }}</dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('operations.sessions.details.timedOut') }}
            </dt>
            <dd>
              {{
                sessionDetail.timed_out
                  ? t('operations.yes')
                  : t('operations.no')
              }}
            </dd>
          </div>
        </dl>
      </template>
    </UDrawer>
  </div>
</template>
