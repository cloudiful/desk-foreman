<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
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
import type {
  CreateRunnerManagerResponse,
  RunnerManagerResponse,
} from '../generated/openapi/types.gen'

const { success } = useNotify()

const rows = ref<RunnerManagerResponse[]>([])
const loading = ref(false)
const error = ref('')
const search = ref('')
let loadSequence = 0

const filtered = computed(() => {
  const query = search.value.trim().toLowerCase()
  if (!query) return rows.value
  return rows.value.filter(
    (row) =>
      row.name.toLowerCase().includes(query) ||
      row.endpoint.toLowerCase().includes(query),
  )
})

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
    const result = await listAdminRunnerManagers()
    if (sequence === loadSequence) rows.value = result
  } catch (err) {
    if (sequence === loadSequence) {
      error.value =
        err instanceof Error ? err.message : 'Failed to load runner managers'
    }
  } finally {
    if (sequence === loadSequence) loading.value = false
  }
}

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
    formError.value = 'Name is required'
    return
  }
  if (!editing.value.endpoint.trim()) {
    formError.value = 'Endpoint is required'
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
      success('Runner manager registered', editing.value.name.trim())
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
      success('Runner manager updated', editing.value.name.trim())
    }
    drawerOpen.value = false
    editing.value = null
    await load()
  } catch (err) {
    formError.value =
      err instanceof Error ? err.message : 'Failed to save runner manager'
  } finally {
    saving.value = false
  }
}

onMounted(() => void load())
</script>

<template>
  <div class="space-y-6">
    <PageHeader
      title="Runner managers"
      description="Execution agents that pull and run workspace jobs"
    >
      <template #actions>
        <UButton
          icon="i-lucide-refresh-cw"
          variant="outline"
          color="neutral"
          :loading="loading"
          @click="load"
        />
        <UButton icon="i-lucide-plus" @click="openCreate">
          Add runner manager
        </UButton>
      </template>
    </PageHeader>

    <div class="grid gap-4 sm:grid-cols-3">
      <StatCard
        title="Online"
        :value="stats.online"
        icon="i-lucide-circle-check"
      />
      <StatCard
        title="Offline"
        :value="stats.offline"
        icon="i-lucide-circle-alert"
      />
      <StatCard
        title="Disabled"
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
          placeholder="Search by name or endpoint…"
          leading-icon="i-lucide-search"
          class="md:max-w-xs"
        />
      </div>

      <ErrorAlert v-if="error" :error="error" class="m-4" @retry="load" />

      <DataTable
        :rows="filtered"
        :columns="[
          { key: 'name', label: 'Manager' },
          { key: 'status', label: 'Status' },
          { key: 'limits', label: 'Limits' },
          { key: 'last_seen_at', label: 'Last seen' },
          { key: 'actions', label: '', class: 'text-right' },
        ]"
        :loading="loading"
        :row-key="(row) => row.runner_manager_id as number"
        :empty-title="search ? 'No matching managers' : 'No runner managers'"
        :empty-description="
          search
            ? 'Try a different search.'
            : 'Register a runner manager to execute workspace jobs.'
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
            {{ row.max_sessions }} sessions ·
            {{ formatMilliseconds(row.max_timeout_ms as number) }} timeout ·
            {{ formatBytes(row.max_output_bytes as number) }} output
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
              aria-label="Edit runner manager"
              @click="openEdit(row as unknown as RunnerManagerResponse)"
            />
          </div>
        </template>
      </DataTable>
    </section>

    <!-- Create/edit drawer -->
    <UDrawer
      v-model:open="drawerOpen"
      :title="editingId === null ? 'Create runner manager' : 'Edit runner manager'"
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
              Connection
            </div>
            <UFormField label="Name">
              <UInput
                v-model="editing.name"
                placeholder="e.g. homelab-docker"
                :disabled="editingId !== null"
                class="w-full"
              />
            </UFormField>
            <UFormField label="Endpoint">
              <UInput
                v-model="editing.endpoint"
                placeholder="http://runner-manager:3001"
                class="w-full"
              />
            </UFormField>
            <UFormField
              label="Host workspace root"
              hint="Optional: host path for runner workspace bind mounts; must match the workspace directory mounted into the runner manager"
            >
              <UInput
                v-model="editing.host_workspace_root"
                placeholder="/opt/desk-foreman/vol/workspace"
                class="w-full"
              />
            </UFormField>
            <UFormField
              v-if="editingId === null"
              label="Existing token"
              hint="Optional: reuse a previously issued runner token"
            >
              <UInput
                v-model="editing.access_token"
                type="password"
                placeholder="Leave blank to issue a new token"
                autocomplete="off"
                class="w-full"
              />
            </UFormField>
            <div
              class="flex items-center justify-between rounded-lg border border-(--ui-border) p-3"
            >
              <div>
                <div class="text-sm font-medium text-(--ui-text-highlighted)">
                  Enabled
                </div>
                <div class="text-xs text-(--ui-text-muted)">
                  Disabled managers stop receiving jobs
                </div>
              </div>
              <USwitch v-model="enabled" />
            </div>
          </div>

          <div class="space-y-4">
            <div
              class="text-xs font-semibold uppercase tracking-wide text-(--ui-text-muted)"
            >
              Container
            </div>
            <UFormField label="Runner image">
              <UInput
                v-model="editing.image"
                placeholder="desk-foreman-workspace-runner:local"
                class="w-full"
              />
            </UFormField>
            <div
              class="flex items-center justify-between rounded-lg border border-(--ui-border) p-3"
            >
              <div>
                <div class="text-sm font-medium text-(--ui-text-highlighted)">
                  Network access
                </div>
                <div class="text-xs text-(--ui-text-muted)">
                  Allow outbound network in runner containers
                </div>
              </div>
              <USwitch v-model="editing.network_enabled" />
            </div>
            <p class="text-xs text-(--ui-text-dimmed)">
              Image and network changes apply to newly started runner containers
              only.
            </p>
          </div>

          <div class="space-y-4">
            <div
              class="text-xs font-semibold uppercase tracking-wide text-(--ui-text-muted)"
            >
              Resource limits
            </div>
            <div class="grid grid-cols-2 gap-3">
              <UFormField label="Max sessions">
                <UInput
                  v-model.number="editing.max_sessions"
                  type="number"
                  min="1"
                  class="w-full"
                />
              </UFormField>
              <UFormField label="Max output (bytes)">
                <UInput
                  v-model.number="editing.max_output_bytes"
                  type="number"
                  min="0"
                  class="w-full"
                />
              </UFormField>
              <UFormField label="Max timeout (ms)">
                <UInput
                  v-model.number="editing.max_timeout_ms"
                  type="number"
                  min="0"
                  class="w-full"
                />
              </UFormField>
              <UFormField label="PID limit">
                <UInput
                  v-model.number="editing.pids_limit"
                  type="number"
                  min="0"
                  class="w-full"
                />
              </UFormField>
              <UFormField label="Memory limit">
                <UInput
                  v-model="editing.memory_limit"
                  placeholder="1g"
                  class="w-full"
                />
              </UFormField>
              <UFormField label="CPU limit">
                <UInput
                  v-model="editing.cpu_limit"
                  placeholder="2"
                  class="w-full"
                />
              </UFormField>
            </div>
            <p class="text-xs text-(--ui-text-dimmed)">
              Timeout and output limits are enforced on every job, old and new.
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
            Cancel
          </UButton>
          <UButton
            type="submit"
            form="runner-manager-form"
            :loading="saving"
            :disabled="saving"
          >
            {{ editingId === null ? 'Register manager' : 'Save changes' }}
          </UButton>
        </div>
      </template>
    </UDrawer>

    <!-- Token reveal modal -->
    <UModal
      v-model:open="tokenModalOpen"
      title="Runner token"
      description="The runner manager needs this token to authenticate with the control plane."
      @update:open="(open) => { if (!open) createdToken = null }"
    >
      <template #body>
        <div class="space-y-4">
          <UAlert
            title="Copy this token now"
            description="It is shown only once and cannot be retrieved later."
            color="warning"
            variant="subtle"
          />
          <TokenReveal v-if="createdToken?.token" :token="createdToken.token" />
          <p class="text-xs text-(--ui-text-muted)">
            Configure it on the runner manager as
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
          >Done</UButton
        >
      </template>
    </UModal>
  </div>
</template>
