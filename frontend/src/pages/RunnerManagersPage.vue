<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  createAdminRunnerManager,
  listAdminRunnerManagers,
  updateAdminRunnerManager,
} from '../api/users'
import { useNotify } from '../composables/useNotify'
import {
  formatBytes,
  formatMilliseconds,
  formatRelative,
} from '../utils/format'
import { PAGE_SIZE, pageCount, pageOffset } from '../utils/pagination'
import type {
  CreateRunnerManagerResponse,
  ListRunnerManagersData,
  RunnerManagerResponse,
} from '../generated/openapi/types.gen'

const { success } = useNotify()
const { t } = useI18n()

const rows = ref<RunnerManagerResponse[]>([])
const total = ref(0)
const loading = ref(false)
const error = ref('')
const search = ref('')
const page = ref(1)
let loadSequence = 0

const totalPages = computed(() => pageCount(total.value, PAGE_SIZE))

const stats = computed(() => {
  const all = rows.value
  return {
    online: all.filter((m) => m.enabled && m.status === 'online').length,
    offline: all.filter((m) => m.enabled && m.status !== 'online').length,
    disabled: all.filter((m) => !m.enabled).length,
  }
})

async function load(): Promise<void> {
  const sequence = ++loadSequence
  loading.value = true
  error.value = ''
  try {
    const query: NonNullable<ListRunnerManagersData['query']> = {
      limit: PAGE_SIZE,
      offset: pageOffset(page.value, PAGE_SIZE),
      search: search.value.trim() || undefined,
    }
    const result = await listAdminRunnerManagers(query)
    if (sequence !== loadSequence) return
    rows.value = result.items
    total.value = result.total
  } catch (err) {
    if (sequence === loadSequence) {
      error.value =
        err instanceof Error ? err.message : t('runnerManagers.errors.load')
    }
  } finally {
    if (sequence === loadSequence) loading.value = false
  }
}

function onPageChange(): void {
  void load()
}

let filterTimer: ReturnType<typeof setTimeout> | undefined
watch(search, () => {
  page.value = 1
  if (filterTimer) clearTimeout(filterTimer)
  filterTimer = setTimeout(() => void load(), 250)
})

onUnmounted(() => {
  if (filterTimer) clearTimeout(filterTimer)
})

interface RunnerForm {
  name: string
  endpoint: string
  access_token: string
  image: string
  host_workspace_root: string
  network_enabled: boolean
  max_output_bytes: number | string
  max_timeout_ms: number | string
  max_sessions: number | string
  pids_limit: number | string
  memory_limit: string
  cpu_limit: string
}

const editing = ref<RunnerForm | null>(null)
const editingId = ref<number | null>(null)
const enabled = ref(true)
const drawerOpen = ref(false)
const saving = ref(false)
const formError = ref('')
const createdToken = ref<CreateRunnerManagerResponse | null>(null)
const tokenModalOpen = ref(false)

const DEFAULT_ENDPOINT = 'http://desk-foreman-runner-manager:3001'

function blankForm(): RunnerForm {
  return {
    name: '',
    endpoint: DEFAULT_ENDPOINT,
    access_token: '',
    image: 'desk-foreman-workspace-runner:local',
    host_workspace_root: '',
    network_enabled: false,
    max_output_bytes: 262144,
    max_timeout_ms: 600000,
    max_sessions: 32,
    pids_limit: 256,
    memory_limit: '1g',
    cpu_limit: '2',
  }
}

function openCreate(): void {
  formError.value = ''
  createdToken.value = null
  tokenModalOpen.value = false
  editingId.value = null
  enabled.value = true
  editing.value = blankForm()
  drawerOpen.value = true
}

function openEdit(row: RunnerManagerResponse): void {
  formError.value = ''
  createdToken.value = null
  tokenModalOpen.value = false
  editingId.value = row.runner_manager_id
  enabled.value = row.enabled
  editing.value = {
    name: row.name,
    endpoint: row.endpoint,
    access_token: '',
    image: row.image,
    host_workspace_root: row.host_workspace_root ?? '',
    network_enabled: row.network_enabled,
    max_output_bytes: row.max_output_bytes,
    max_timeout_ms: row.max_timeout_ms,
    max_sessions: row.max_sessions,
    pids_limit: row.pids_limit,
    memory_limit: row.memory_limit,
    cpu_limit: row.cpu_limit,
  }
  drawerOpen.value = true
}

function handleRunnerDrawerOpen(open: boolean): void {
  if (!open && !saving.value) {
    editing.value = null
    editingId.value = null
    formError.value = ''
  }
}

function toNumber(
  value: number | string | null | undefined,
  fallback: number,
): number {
  if (value === '' || value === null || value === undefined) return fallback
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : fallback
}

async function save(): Promise<void> {
  if (!editing.value || saving.value) return
  if (!editing.value.name.trim()) {
    formError.value = t('runnerManagers.validation.nameRequired')
    return
  }
  if (!editing.value.endpoint.trim()) {
    formError.value = t('runnerManagers.validation.endpointRequired')
    return
  }
  saving.value = true
  formError.value = ''
  try {
    if (editingId.value === null) {
      const result = await createAdminRunnerManager({
        name: editing.value.name.trim(),
        endpoint: editing.value.endpoint.trim(),
        access_token: editing.value.access_token.trim() || null,
        image: editing.value.image.trim(),
        host_workspace_root: editing.value.host_workspace_root.trim() || null,
        network_enabled: editing.value.network_enabled,
        max_output_bytes: toNumber(editing.value.max_output_bytes, 262144),
        max_timeout_ms: toNumber(editing.value.max_timeout_ms, 600000),
        max_sessions: toNumber(editing.value.max_sessions, 32),
        pids_limit: toNumber(editing.value.pids_limit, 256),
        memory_limit: editing.value.memory_limit.trim() || '1g',
        cpu_limit: editing.value.cpu_limit.trim() || '2',
      })
      success(
        t('runnerManagers.notifications.registered'),
        editing.value.name.trim(),
      )
      if (result.token) {
        createdToken.value = result
        tokenModalOpen.value = true
      }
    } else {
      await updateAdminRunnerManager(editingId.value, {
        endpoint: editing.value.endpoint.trim(),
        enabled: enabled.value,
        image: editing.value.image.trim(),
        host_workspace_root: editing.value.host_workspace_root.trim() || null,
        network_enabled: editing.value.network_enabled,
        max_output_bytes: toNumber(editing.value.max_output_bytes, 262144),
        max_timeout_ms: toNumber(editing.value.max_timeout_ms, 600000),
        max_sessions: toNumber(editing.value.max_sessions, 32),
        pids_limit: toNumber(editing.value.pids_limit, 256),
        memory_limit: editing.value.memory_limit.trim() || '1g',
        cpu_limit: editing.value.cpu_limit.trim() || '2',
      })
      success(
        t('runnerManagers.notifications.updated'),
        editing.value.name.trim(),
      )
    }
    drawerOpen.value = false
    editing.value = null
    await load()
  } catch (err) {
    formError.value =
      err instanceof Error ? err.message : t('runnerManagers.errors.save')
  } finally {
    saving.value = false
  }
}

onMounted(() => void load())
</script>

<template>
  <div class="space-y-6">
    <PageHeader>
      <template #actions>
        <UButton
          icon="i-lucide-refresh-cw"
          variant="outline"
          color="neutral"
          :loading="loading"
          :aria-label="t('runnerManagers.actions.refresh')"
          @click="load"
        />
        <UButton icon="i-lucide-plus" @click="openCreate">
          {{ t('runnerManagers.actions.add') }}
        </UButton>
      </template>
    </PageHeader>

    <div class="grid gap-4 sm:grid-cols-3">
      <StatCard
        :title="t('runnerManagers.stats.online')"
        :value="stats.online"
        icon="i-lucide-circle-check"
      />
      <StatCard
        :title="t('runnerManagers.stats.offline')"
        :value="stats.offline"
        icon="i-lucide-circle-alert"
      />
      <StatCard
        :title="t('runnerManagers.stats.disabled')"
        :value="stats.disabled"
        icon="i-lucide-circle-off"
      />
    </div>

    <section
      class="rounded-xl border border-(--ui-border) bg-(--ui-bg) shadow-sm"
    >
      <div class="border-b border-(--ui-border) p-4">
        <UInput
          v-model="search"
          :placeholder="t('runnerManagers.searchPlaceholder')"
          leading-icon="i-lucide-search"
          class="md:max-w-xs"
        />
      </div>

      <ErrorAlert v-if="error" :error="error" class="m-4" @retry="load" />

      <DataTable
        :rows="rows"
        :columns="[
          { key: 'name', label: t('runnerManagers.table.manager') },
          { key: 'status', label: t('runnerManagers.table.status') },
          { key: 'limits', label: t('runnerManagers.table.limits') },
          { key: 'last_seen_at', label: t('runnerManagers.table.lastSeen') },
          { key: 'actions', label: '', class: 'text-right' },
        ]"
        :loading="loading"
        :row-key="(row) => row.runner_manager_id as number"
        :empty-title="
          search
            ? t('runnerManagers.empty.matchingTitle')
            : t('runnerManagers.empty.title')
        "
        :empty-description="
          search
            ? t('runnerManagers.empty.searchDescription')
            : t('runnerManagers.empty.description')
        "
      >
        <template #cell-name="{ row }">
          <div class="min-w-0">
            <div class="flex items-center gap-2">
              <UIcon
                name="i-lucide-server-cog"
                class="size-4 shrink-0 text-(--ui-text-dimmed)"
              />
              <span class="truncate font-medium text-(--ui-text-highlighted)">
                {{ row.name }}
              </span>
            </div>
            <div
              class="mt-0.5 truncate font-mono text-xs text-(--ui-text-muted)"
            >
              {{ row.endpoint }}
            </div>
            <div class="truncate text-xs text-(--ui-text-dimmed)">
              {{ row.image }}
            </div>
          </div>
        </template>
        <template #cell-status="{ row }">
          <StatusBadge
            :status="
              !(row.enabled as boolean)
                ? 'disabled'
                : (row.status as string) === 'online'
                  ? 'online'
                  : 'offline'
            "
          />
        </template>
        <template #cell-limits="{ row }">
          <span class="text-xs text-(--ui-text-muted)">
            {{
              t('runnerManagers.limits', {
                sessions: row.max_sessions,
                timeout: formatMilliseconds(row.max_timeout_ms as number),
                output: formatBytes(row.max_output_bytes as number),
              })
            }}
          </span>
        </template>
        <template #cell-last_seen_at="{ row }">
          <span class="whitespace-nowrap text-sm text-(--ui-text-muted)">
            {{ formatRelative(row.last_seen_at as string | null) }}
          </span>
        </template>
        <template #cell-actions="{ row }">
          <div class="flex justify-end">
            <UButton
              icon="i-lucide-pencil"
              variant="ghost"
              color="neutral"
              size="sm"
              :aria-label="t('runnerManagers.actions.edit')"
              @click="openEdit(row as unknown as RunnerManagerResponse)"
            />
          </div>
        </template>
      </DataTable>

      <div
        v-if="totalPages > 1"
        class="flex items-center justify-between border-t border-(--ui-border) px-4 py-3"
      >
        <span class="text-sm text-(--ui-text-muted)">
          {{ t('runnerManagers.pagination', { page, totalPages }) }}
        </span>
        <UPagination
          v-model:page="page"
          :total="total"
          :items-per-page="PAGE_SIZE"
          @update:page="onPageChange"
        />
      </div>
    </section>

    <!-- Create/edit drawer -->
    <UDrawer
      v-model:open="drawerOpen"
      :title="
        t(
          editingId === null
            ? 'runnerManagers.drawer.createTitle'
            : 'runnerManagers.drawer.editTitle',
        )
      "
      :dismissible="!saving"
      @update:open="handleRunnerDrawerOpen"
    >
      <template #body>
        <form
          v-if="editing"
          id="runner-manager-form"
          class="mx-auto w-full max-w-5xl space-y-6"
          @submit.prevent="save"
        >
          <div class="space-y-4">
            <div
              class="text-xs font-semibold uppercase tracking-wide text-(--ui-text-muted)"
            >
              {{ t('runnerManagers.sections.connection') }}
            </div>
            <UFormField :label="t('runnerManagers.fields.name')">
              <UInput
                v-model="editing.name"
                :placeholder="t('runnerManagers.placeholders.name')"
                :disabled="editingId !== null"
                class="w-full"
              />
            </UFormField>
            <UFormField :label="t('runnerManagers.fields.endpoint')">
              <UInput
                v-model="editing.endpoint"
                :placeholder="t('runnerManagers.placeholders.endpoint')"
                class="w-full"
              />
            </UFormField>
            <UFormField
              :label="t('runnerManagers.fields.hostWorkspaceRoot')"
              :hint="t('runnerManagers.hints.hostWorkspaceRoot')"
            >
              <UInput
                v-model="editing.host_workspace_root"
                :placeholder="
                  t('runnerManagers.placeholders.hostWorkspaceRoot')
                "
                class="w-full"
              />
            </UFormField>
            <UFormField
              v-if="editingId === null"
              :label="t('runnerManagers.fields.existingToken')"
              :hint="t('runnerManagers.hints.existingToken')"
            >
              <UInput
                v-model="editing.access_token"
                type="password"
                :placeholder="t('runnerManagers.placeholders.existingToken')"
                autocomplete="off"
                class="w-full"
              />
            </UFormField>
            <div
              class="flex items-center justify-between rounded-lg border border-(--ui-border) p-3"
            >
              <div>
                <div class="text-sm font-medium text-(--ui-text-highlighted)">
                  {{ t('runnerManagers.fields.enabled') }}
                </div>
                <div class="text-xs text-(--ui-text-muted)">
                  {{ t('runnerManagers.hints.enabled') }}
                </div>
              </div>
              <USwitch v-model="enabled" />
            </div>
          </div>

          <div class="space-y-4">
            <div
              class="text-xs font-semibold uppercase tracking-wide text-(--ui-text-muted)"
            >
              {{ t('runnerManagers.sections.container') }}
            </div>
            <UFormField :label="t('runnerManagers.fields.runnerImage')">
              <UInput
                v-model="editing.image"
                :placeholder="t('runnerManagers.placeholders.runnerImage')"
                class="w-full"
              />
            </UFormField>
            <div
              class="flex items-center justify-between rounded-lg border border-(--ui-border) p-3"
            >
              <div>
                <div class="text-sm font-medium text-(--ui-text-highlighted)">
                  {{ t('runnerManagers.fields.networkAccess') }}
                </div>
                <div class="text-xs text-(--ui-text-muted)">
                  {{ t('runnerManagers.hints.networkAccess') }}
                </div>
              </div>
              <USwitch v-model="editing.network_enabled" />
            </div>
            <p class="text-xs text-(--ui-text-dimmed)">
              {{ t('runnerManagers.hints.imageAndNetwork') }}
            </p>
          </div>

          <div class="space-y-4">
            <div
              class="text-xs font-semibold uppercase tracking-wide text-(--ui-text-muted)"
            >
              {{ t('runnerManagers.sections.resourceLimits') }}
            </div>
            <div class="grid grid-cols-2 gap-3">
              <UFormField :label="t('runnerManagers.fields.maxSessions')">
                <UInput
                  v-model.number="editing.max_sessions"
                  type="number"
                  min="1"
                  class="w-full"
                />
              </UFormField>
              <UFormField :label="t('runnerManagers.fields.maxOutput')">
                <UInput
                  v-model.number="editing.max_output_bytes"
                  type="number"
                  min="0"
                  class="w-full"
                />
              </UFormField>
              <UFormField :label="t('runnerManagers.fields.maxTimeout')">
                <UInput
                  v-model.number="editing.max_timeout_ms"
                  type="number"
                  min="0"
                  class="w-full"
                />
              </UFormField>
              <UFormField :label="t('runnerManagers.fields.pidLimit')">
                <UInput
                  v-model.number="editing.pids_limit"
                  type="number"
                  min="0"
                  class="w-full"
                />
              </UFormField>
              <UFormField :label="t('runnerManagers.fields.memoryLimit')">
                <UInput
                  v-model="editing.memory_limit"
                  :placeholder="t('runnerManagers.placeholders.memoryLimit')"
                  class="w-full"
                />
              </UFormField>
              <UFormField :label="t('runnerManagers.fields.cpuLimit')">
                <UInput
                  v-model="editing.cpu_limit"
                  :placeholder="t('runnerManagers.placeholders.cpuLimit')"
                  class="w-full"
                />
              </UFormField>
            </div>
            <p class="text-xs text-(--ui-text-dimmed)">
              {{ t('runnerManagers.hints.limits') }}
            </p>
          </div>

          <UAlert
            v-if="formError"
            :title="formError"
            color="error"
            variant="subtle"
          />
        </form>
      </template>
      <template #footer>
        <div class="mx-auto flex w-full max-w-5xl justify-end gap-2">
          <UButton
            variant="outline"
            color="neutral"
            :disabled="saving"
            @click="
              () => {
                drawerOpen = false
              }
            "
          >
            {{ t('runnerManagers.actions.cancel') }}
          </UButton>
          <UButton
            type="submit"
            form="runner-manager-form"
            :loading="saving"
            :disabled="saving"
          >
            {{
              t(
                editingId === null
                  ? 'runnerManagers.actions.register'
                  : 'runnerManagers.actions.save',
              )
            }}
          </UButton>
        </div>
      </template>
    </UDrawer>

    <!-- Token reveal modal -->
    <UModal
      v-model:open="tokenModalOpen"
      :title="t('runnerManagers.token.title')"
      :description="t('runnerManagers.token.description')"
      @update:open="
        (open) => {
          if (!open) createdToken = null
        }
      "
    >
      <template #body>
        <div class="space-y-4">
          <UAlert
            :title="t('runnerManagers.token.copyTitle')"
            :description="t('runnerManagers.token.copyDescription')"
            color="warning"
            variant="subtle"
          />
          <TokenReveal v-if="createdToken?.token" :token="createdToken.token" />
          <p class="text-xs text-(--ui-text-muted)">
            {{ t('runnerManagers.token.configure') }}
            <code class="font-mono">RUNNER_MANAGER_TOKEN</code>.
          </p>
        </div>
      </template>
      <template #footer>
        <UButton
          block
          @click="
            () => {
              tokenModalOpen = false
            }
          "
          >{{ t('runnerManagers.actions.done') }}</UButton
        >
      </template>
    </UModal>
  </div>
</template>
