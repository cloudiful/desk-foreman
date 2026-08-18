<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import {
  createAdminApplication,
  createAdminApplicationToken,
  deleteAdminApplicationToken,
  listAdminApplicationTokens,
  listAdminApplications,
  testAdminApplicationApproval,
  updateAdminApplication,
} from '../api/users'
import { useNotify } from '../composables/useNotify'
import {
  formatBytes,
  formatDateTime,
  formatMilliseconds,
  formatRelative,
} from '../utils/format'
import type {
  ApplicationResponse,
  ApplicationTokenResponse,
  CreateApplicationTokenResponse,
  ApprovalTestResponse,
} from '../generated/openapi/types.gen'

const AVAILABLE_SCOPES = [
  'workspace.read',
  'workspace.search',
  'workspace.shell',
  'workspace.patch',
] as const
type AvailableScope = (typeof AVAILABLE_SCOPES)[number]

function availableScopes(scopes: string[]): AvailableScope[] {
  return scopes.filter((scope): scope is AvailableScope =>
    AVAILABLE_SCOPES.includes(scope as AvailableScope),
  )
}

function unknownScopes(scopes: string[]): string[] {
  return scopes.filter(
    (scope) => !AVAILABLE_SCOPES.includes(scope as AvailableScope),
  )
}

const APPROVAL_MODES = [
  { label: 'Inherit global settings', value: 'inherit' },
  { label: 'Disabled', value: 'disabled' },
  { label: 'Use application reviewer', value: 'enabled' },
] as const

const { success, error: notifyError } = useNotify()

const rows = ref<ApplicationResponse[]>([])
const loading = ref(false)
const error = ref('')
const search = ref('')
const statusFilter = ref<'all' | 'active' | 'inactive'>('all')

const filtered = computed(() => {
  const query = search.value.trim().toLowerCase()
  return rows.value.filter((row) => {
    if (statusFilter.value === 'active' && !row.is_active) return false
    if (statusFilter.value === 'inactive' && row.is_active) return false
    if (!query) return true
    return row.name.toLowerCase().includes(query)
  })
})

async function load(): Promise<void> {
  const sequence = ++loadSequence
  loading.value = true
  error.value = ''
  try {
    const result = await listAdminApplications()
    if (sequence === loadSequence) rows.value = result
  } catch (err) {
    if (sequence === loadSequence) {
      error.value =
        err instanceof Error ? err.message : 'Failed to load applications'
    }
  } finally {
    if (sequence === loadSequence) loading.value = false
  }
}

// ----- create/edit -----
interface ApplicationForm {
  application_id: number
  name: string
  workspace_template: string
  default_shell: string
  default_scopes: AvailableScope[]
  max_timeout_ms: number | string
  max_output_bytes: number | string
  max_file_bytes: number | string
  max_sessions: number | string
  network_enabled: boolean
  is_active: boolean
  approval_mode: string
  approval_endpoint: string
  approval_model: string
  approval_timeout_ms: number | string
  approval_max_input_bytes: number | string
  approval_max_concurrent: number | string
  approval_max_output_tokens: number | string
  approval_api_key: string
  approval_api_key_configured: boolean
  clear_approval_api_key: boolean
}

const editing = ref<ApplicationForm | null>(null)
const editingUnknownScopes = ref<string[]>([])
const drawerOpen = ref(false)
const saving = ref(false)
const formError = ref('')
const approvalTesting = ref(false)
const approvalTestResult = ref<ApprovalTestResponse | null>(null)
let loadSequence = 0

function toNumberOrNull(
  value: number | string | null | undefined,
): number | null {
  if (value === null || value === undefined || value === '') return null
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : null
}

function blankForm(): ApplicationForm {
  return {
    application_id: 0,
    name: '',
    workspace_template: '',
    default_shell: '/bin/bash',
    default_scopes: [...AVAILABLE_SCOPES],
    max_timeout_ms: '',
    max_output_bytes: '',
    max_file_bytes: '',
    max_sessions: '',
    network_enabled: true,
    is_active: true,
    approval_mode: 'inherit',
    approval_endpoint: '',
    approval_model: '',
    approval_timeout_ms: '',
    approval_max_input_bytes: '',
    approval_max_concurrent: '',
    approval_max_output_tokens: '',
    approval_api_key: '',
    approval_api_key_configured: false,
    clear_approval_api_key: false,
  }
}

function startCreate(): void {
  formError.value = ''
  approvalTestResult.value = null
  editingUnknownScopes.value = []
  editing.value = blankForm()
  drawerOpen.value = true
}

function startEdit(row: ApplicationResponse): void {
  formError.value = ''
  editing.value = {
    application_id: row.application_id,
    name: row.name,
    workspace_template: row.workspace_template ?? '',
    default_shell: row.default_shell ?? '',
    default_scopes: availableScopes(row.default_scopes),
    max_timeout_ms: row.max_timeout_ms ?? '',
    max_output_bytes: row.max_output_bytes ?? '',
    max_file_bytes: row.max_file_bytes ?? '',
    max_sessions: row.max_sessions ?? '',
    network_enabled: row.network_enabled,
    is_active: row.is_active,
    approval_mode: row.approval_mode,
    approval_endpoint: row.approval_endpoint ?? '',
    approval_model: row.approval_model ?? '',
    approval_timeout_ms: row.approval_timeout_ms ?? '',
    approval_max_input_bytes: row.approval_max_input_bytes ?? '',
    approval_max_concurrent: row.approval_max_concurrent ?? '',
    approval_max_output_tokens: row.approval_max_output_tokens ?? '',
    approval_api_key: '',
    approval_api_key_configured: row.approval_api_key_configured,
    clear_approval_api_key: false,
  }
  editingUnknownScopes.value = unknownScopes(row.default_scopes)
  approvalTestResult.value = null
  drawerOpen.value = true
}

function handleApplicationDrawerOpen(open: boolean): void {
  if (!open && !saving.value) {
    editing.value = null
    editingUnknownScopes.value = []
    formError.value = ''
    approvalTestResult.value = null
  }
}

async function save(): Promise<void> {
  if (!editing.value || saving.value) return
  if (!editing.value.name.trim()) {
    formError.value = 'Name is required'
    return
  }
  saving.value = true
  formError.value = ''
  const body = {
    name: editing.value.name.trim(),
    workspace_template: editing.value.workspace_template.trim() || null,
    default_shell: editing.value.default_shell.trim() || null,
    default_scopes: [
      ...editing.value.default_scopes,
      ...editingUnknownScopes.value,
    ],
    max_timeout_ms: toNumberOrNull(editing.value.max_timeout_ms),
    max_output_bytes: toNumberOrNull(editing.value.max_output_bytes),
    max_file_bytes: toNumberOrNull(editing.value.max_file_bytes),
    max_sessions: toNumberOrNull(editing.value.max_sessions),
    network_enabled: editing.value.network_enabled,
    approval_mode: editing.value.approval_mode,
    approval_endpoint: editing.value.approval_endpoint.trim() || null,
    approval_model: editing.value.approval_model.trim() || null,
    approval_timeout_ms: toNumberOrNull(editing.value.approval_timeout_ms),
    approval_max_input_bytes: toNumberOrNull(
      editing.value.approval_max_input_bytes,
    ),
    approval_max_concurrent: toNumberOrNull(
      editing.value.approval_max_concurrent,
    ),
    approval_max_output_tokens: toNumberOrNull(
      editing.value.approval_max_output_tokens,
    ),
    approval_api_key: editing.value.approval_api_key.trim() || null,
    clear_approval_api_key: editing.value.clear_approval_api_key,
  }
  try {
    if (editing.value.application_id === 0) {
      await createAdminApplication(body)
      success('Application created', body.name)
    } else {
      await updateAdminApplication(editing.value.application_id, {
        ...body,
        is_active: editing.value.is_active,
      })
      success('Application updated', body.name)
    }
    drawerOpen.value = false
    editing.value = null
    await load()
  } catch (err) {
    formError.value =
      err instanceof Error ? err.message : 'Failed to save application'
  } finally {
    saving.value = false
  }
}

async function testApplicationApproval(): Promise<void> {
  if (
    !editing.value ||
    editing.value.application_id === 0 ||
    approvalTesting.value
  )
    return
  approvalTesting.value = true
  formError.value = ''
  try {
    approvalTestResult.value = await testAdminApplicationApproval(
      editing.value.application_id,
    )
  } catch (err) {
    formError.value =
      err instanceof Error ? err.message : 'Failed to test application reviewer'
  } finally {
    approvalTesting.value = false
  }
}

function clearApplicationKey(): void {
  if (!editing.value) return
  editing.value.approval_api_key = ''
  editing.value.clear_approval_api_key = true
}

// ----- tokens -----
const tokenOwner = ref<ApplicationResponse | null>(null)
const allTokens = ref<ApplicationTokenResponse[]>([])
const tokenDrawerOpen = ref(false)
const tokenName = ref('')
const tokenScopes = ref<AvailableScope[]>([...AVAILABLE_SCOPES])
const tokenExpiresAt = ref('')
const creatingToken = ref(false)
const revealedToken = ref<CreateApplicationTokenResponse | null>(null)
const revokeTarget = ref<ApplicationTokenResponse | null>(null)
const revoking = ref(false)
let tokenLoadSequence = 0

const visibleTokens = computed(() =>
  tokenOwner.value
    ? allTokens.value.filter(
        (t) => t.application_id === tokenOwner.value?.application_id,
      )
    : [],
)

const revokeModalOpen = computed<boolean>({
  get: () => Boolean(revokeTarget.value),
  set: (open) => {
    if (!open) revokeTarget.value = null
  },
})

async function openTokenDrawer(row: ApplicationResponse): Promise<void> {
  tokenOwner.value = row
  tokenName.value = ''
  tokenScopes.value = [...AVAILABLE_SCOPES]
  tokenExpiresAt.value = ''
  revealedToken.value = null
  tokenDrawerOpen.value = true
  await loadTokens()
}

function handleTokenDrawerOpen(open: boolean): void {
  if (!open) {
    revealedToken.value = null
    tokenOwner.value = null
  }
}

async function loadTokens(): Promise<void> {
  const sequence = ++tokenLoadSequence
  try {
    const result = await listAdminApplicationTokens()
    if (sequence === tokenLoadSequence) allTokens.value = result
  } catch (err) {
    notifyError(
      'Failed to load application tokens',
      err instanceof Error ? err.message : undefined,
    )
  }
}

async function createToken(): Promise<void> {
  if (!tokenOwner.value || !tokenName.value.trim() || creatingToken.value) return
  creatingToken.value = true
  try {
    revealedToken.value = await createAdminApplicationToken({
      application_id: tokenOwner.value.application_id,
      token_name: tokenName.value.trim(),
      scopes: tokenScopes.value,
      expires_at: tokenExpiresAt.value
        ? new Date(tokenExpiresAt.value).toISOString()
        : null,
    })
    tokenName.value = ''
    await loadTokens()
    success('Token created')
  } catch (err) {
    notifyError(
      'Failed to create token',
      err instanceof Error ? err.message : undefined,
    )
  } finally {
    creatingToken.value = false
  }
}

async function confirmRevokeToken(): Promise<void> {
  if (!revokeTarget.value || revoking.value) return
  revoking.value = true
  try {
    await deleteAdminApplicationToken(revokeTarget.value.token_id)
    success('Token revoked', revokeTarget.value.token_name)
    revokeTarget.value = null
    await loadTokens()
  } catch (err) {
    notifyError(
      'Failed to revoke token',
      err instanceof Error ? err.message : undefined,
    )
  } finally {
    revoking.value = false
  }
}

onMounted(() => void load())
</script>

<template>
  <div class="space-y-6">
    <PageHeader
      title="Applications"
      description="Integrations with workspace access and per-application policies"
    >
      <template #actions>
        <UButton
          icon="i-lucide-refresh-cw"
          variant="outline"
          color="neutral"
          :loading="loading"
          @click="load"
        />
        <UButton icon="i-lucide-plus" @click="startCreate">
          Create application
        </UButton>
      </template>
    </PageHeader>

    <section
      class="rounded-xl border border-(--ui-border) bg-(--ui-bg) shadow-sm"
    >
      <div
        class="flex flex-col gap-3 border-b border-(--ui-border) p-4 md:flex-row md:items-center"
      >
        <UInput
          v-model="search"
          placeholder="Search applications…"
          leading-icon="i-lucide-search"
          class="md:max-w-xs"
        />
        <USelect
          v-model="statusFilter"
          :items="[
            { label: 'All statuses', value: 'all' },
            { label: 'Active', value: 'active' },
            { label: 'Inactive', value: 'inactive' },
          ]"
          class="w-36"
        />
        <span class="ml-auto text-sm text-(--ui-text-muted)">
          {{ filtered.length }} applications
        </span>
      </div>

      <ErrorAlert v-if="error" :error="error" class="m-4" @retry="load" />

      <DataTable
        :rows="filtered"
        :columns="[
          { key: 'name', label: 'Application' },
          { key: 'limits', label: 'Limits' },
          { key: 'scopes', label: 'Scopes' },
          { key: 'status', label: 'Status' },
          { key: 'updated_at', label: 'Updated' },
          { key: 'actions', label: '', class: 'text-right' },
        ]"
        :loading="loading"
        :row-key="(row) => row.application_id as number"
        :empty-title="
          search ? 'No matching applications' : 'No applications yet'
        "
        :empty-description="
          search
            ? 'Try a different search.'
            : 'Create the first application to get started.'
        "
      >
        <template #cell-name="{ row }">
          <div class="flex items-center gap-2">
            <div
              class="flex size-8 items-center justify-center rounded-md bg-(--ui-bg-elevated) text-(--ui-text-muted)"
            >
              <UIcon name="i-lucide-app-window" class="size-4" />
            </div>
            <div class="min-w-0">
              <div class="truncate font-medium text-(--ui-text-highlighted)">
                {{ row.name }}
              </div>
              <div class="truncate font-mono text-xs text-(--ui-text-muted)">
                #{{ row.application_id }} ·
                {{ row.default_shell ?? 'default shell' }}
              </div>
            </div>
          </div>
        </template>
        <template #cell-limits="{ row }">
          <span class="text-xs text-(--ui-text-muted)">
            <template v-if="row.max_timeout_ms">
              timeout {{ formatMilliseconds(row.max_timeout_ms as number)
              }}<br />
            </template>
            <template v-else>no timeout<br /></template>
            <template v-if="row.max_output_bytes">
              output {{ formatBytes(row.max_output_bytes as number) }}
            </template>
            <template v-else>unlimited output</template>
          </span>
        </template>
        <template #cell-scopes="{ row }">
          <UBadge variant="subtle" color="neutral" size="sm">
            {{ (row.default_scopes as string[]).length }} scopes
          </UBadge>
        </template>
        <template #cell-status="{ row }">
          <StatusBadge
            :status="(row.is_active as boolean) ? 'active' : 'inactive'"
          />
        </template>
        <template #cell-updated_at="{ row }">
          <span class="whitespace-nowrap text-sm text-(--ui-text-muted)">
            {{ formatRelative(row.updated_at as string) }}
          </span>
        </template>
        <template #cell-actions="{ row }">
          <div class="flex justify-end gap-1">
            <UButton
              icon="i-lucide-pencil"
              variant="ghost"
              color="neutral"
              size="sm"
              aria-label="Edit application"
              @click="startEdit(row as unknown as ApplicationResponse)"
            />
            <UButton
              icon="i-lucide-key-round"
              variant="ghost"
              color="neutral"
              size="sm"
              aria-label="Application tokens"
              @click="openTokenDrawer(row as unknown as ApplicationResponse)"
            />
          </div>
        </template>
      </DataTable>
    </section>

    <!-- Application drawer -->
    <UDrawer
      v-model:open="drawerOpen"
      :title="editing?.application_id === 0 ? 'Create application' : 'Edit application'"
      :dismissible="!saving"
      @update:open="handleApplicationDrawerOpen"
    >
      <template #body>
        <form
          v-if="editing"
          id="application-form"
          class="space-y-6"
          @submit.prevent="save"
        >
          <div class="space-y-4">
            <div
              class="text-xs font-semibold uppercase tracking-wide text-(--ui-text-muted)"
            >
              General
            </div>
            <UFormField label="Name">
              <UInput v-model="editing.name" placeholder="e.g. code-agent" />
            </UFormField>
            <UFormField
              label="Workspace template"
              hint="Optional template directory copied into new workspaces"
            >
              <UInput
                v-model="editing.workspace_template"
                placeholder="e.g. web-app-template"
              />
            </UFormField>
            <UFormField label="Default shell">
              <UInput v-model="editing.default_shell" placeholder="/bin/bash" />
            </UFormField>
            <UFormField label="Scopes">
              <UCheckboxGroup
                v-model="editing.default_scopes"
                :items="[...AVAILABLE_SCOPES]"
              />
            </UFormField>
          </div>

          <div class="space-y-4">
            <div
              class="text-xs font-semibold uppercase tracking-wide text-(--ui-text-muted)"
            >
              Resource limits
            </div>
            <div class="grid grid-cols-2 gap-3">
              <UFormField label="Max timeout (ms)">
                <UInput
                  v-model.number="editing.max_timeout_ms"
                  type="number"
                  min="0"
                  placeholder="Unlimited"
                />
              </UFormField>
              <UFormField label="Max output (bytes)">
                <UInput
                  v-model.number="editing.max_output_bytes"
                  type="number"
                  min="0"
                  placeholder="Unlimited"
                />
              </UFormField>
              <UFormField label="Max file (bytes)">
                <UInput
                  v-model.number="editing.max_file_bytes"
                  type="number"
                  min="0"
                  placeholder="Unlimited"
                />
              </UFormField>
              <UFormField label="Max sessions">
                <UInput
                  v-model.number="editing.max_sessions"
                  type="number"
                  min="0"
                  placeholder="Unlimited"
                />
              </UFormField>
            </div>
            <div
              class="flex items-center justify-between rounded-lg border border-(--ui-border) p-3"
            >
              <div>
                <div class="text-sm font-medium text-(--ui-text-highlighted)">
                  Network access
                </div>
                <div class="text-xs text-(--ui-text-muted)">
                  Allow outbound network for sessions
                </div>
              </div>
              <USwitch v-model="editing.network_enabled" />
            </div>
            <div
              v-if="editing.application_id !== 0"
              class="flex items-center justify-between rounded-lg border border-(--ui-border) p-3"
            >
              <div>
                <div class="text-sm font-medium text-(--ui-text-highlighted)">
                  Active
                </div>
                <div class="text-xs text-(--ui-text-muted)">
                  Inactive applications cannot authenticate
                </div>
              </div>
              <USwitch v-model="editing.is_active" />
            </div>
          </div>

          <div class="space-y-4">
            <div
              class="text-xs font-semibold uppercase tracking-wide text-(--ui-text-muted)"
            >
              Approval reviewer
            </div>
            <UFormField label="Mode">
              <USelect
                v-model="editing.approval_mode"
                :items="
                  APPROVAL_MODES as unknown as {
                    label: string
                    value: string
                  }[]
                "
                class="w-full"
              />
            </UFormField>
            <template v-if="editing.approval_mode === 'enabled'">
              <UFormField label="Responses API base URL">
                <UInput
                  v-model="editing.approval_endpoint"
                  placeholder="https://api.openai.com/v1"
                />
              </UFormField>
              <UFormField label="Model">
                <UInput
                  v-model="editing.approval_model"
                  placeholder="Reviewer model"
                />
              </UFormField>
              <UFormField
                label="API key"
                hint="Leave blank to keep the stored key"
              >
                <div class="flex gap-2">
                  <UInput
                    v-model="editing.approval_api_key"
                    type="password"
                    autocomplete="new-password"
                    placeholder="Enter an application reviewer key"
                    class="min-w-0 flex-1"
                    @input="editing.clear_approval_api_key = false"
                  />
                  <UButton
                    v-if="editing.approval_api_key_configured"
                    type="button"
                    icon="i-lucide-trash-2"
                    variant="outline"
                    color="error"
                    aria-label="Clear application reviewer API key"
                    @click="clearApplicationKey"
                  />
                </div>
              </UFormField>
              <div class="grid gap-4 sm:grid-cols-3">
                <UFormField label="Timeout (ms)">
                  <UInput
                    v-model.number="editing.approval_timeout_ms"
                    type="number"
                    min="100"
                    max="30000"
                    placeholder="Global default"
                  />
                </UFormField>
                <UFormField label="Max input (bytes)">
                  <UInput
                    v-model.number="editing.approval_max_input_bytes"
                    type="number"
                    min="1"
                    max="524288"
                    placeholder="Global default"
                  />
                </UFormField>
                <UFormField label="Concurrent reviews">
                  <UInput
                    v-model.number="editing.approval_max_concurrent"
                    type="number"
                    min="1"
                    max="64"
                    placeholder="Global default"
                  />
                </UFormField>
                <UFormField label="Max output tokens">
                  <UInput
                    v-model.number="editing.approval_max_output_tokens"
                    type="number"
                    min="256"
                    max="8192"
                    placeholder="Global default"
                  />
                </UFormField>
              </div>
              <div class="flex flex-wrap items-center gap-2">
                <UButton
                  v-if="editing.application_id !== 0"
                  type="button"
                  icon="i-lucide-plug-zap"
                  variant="outline"
                  color="neutral"
                  :loading="approvalTesting"
                  @click="testApplicationApproval"
                >
                  Test application reviewer
                </UButton>
                <span class="text-xs text-(--ui-text-muted)">
                  Tests the saved application configuration without executing a tool.
                </span>
              </div>
              <UAlert
                v-if="approvalTestResult"
                :title="
                  approvalTestResult.ok
                    ? 'Reviewer test passed'
                    : 'Reviewer test failed'
                "
                :description="
                  `${approvalTestResult.message} (${approvalTestResult.latency_ms} ms)`
                "
                :color="approvalTestResult.ok ? 'success' : 'error'"
                variant="subtle"
              />
            </template>
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
        <div class="flex justify-end gap-2">
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
            form="application-form"
            :loading="saving"
            :disabled="saving"
          >
            {{
              editing?.application_id === 0
                ? 'Create application'
                : 'Save changes'
            }}
          </UButton>
        </div>
      </template>
    </UDrawer>

    <!-- Tokens drawer -->
    <UDrawer
      v-model:open="tokenDrawerOpen"
      title="Application tokens"
      @update:open="handleTokenDrawerOpen"
    >
      <template #body>
        <div class="space-y-6">
          <div
            v-if="tokenOwner"
            class="flex items-center gap-3 rounded-lg border border-(--ui-border) p-3"
          >
            <div
              class="flex size-8 items-center justify-center rounded-md bg-(--ui-bg-elevated) text-(--ui-text-muted)"
            >
              <UIcon name="i-lucide-app-window" class="size-4" />
            </div>
            <div>
              <div class="text-sm font-medium text-(--ui-text-highlighted)">
                {{ tokenOwner.name }}
              </div>
              <div class="font-mono text-xs text-(--ui-text-muted)">
                #{{ tokenOwner.application_id }}
              </div>
            </div>
          </div>

          <div class="space-y-4 rounded-lg border border-(--ui-border) p-4">
            <div class="text-sm font-semibold text-(--ui-text-highlighted)">
              Create a token
            </div>
            <UFormField label="Token name">
              <UInput
                v-model="tokenName"
                placeholder="e.g. ci-agent"
                @keyup.enter="createToken"
              />
            </UFormField>
            <UFormField label="Scopes">
              <UCheckboxGroup
                v-model="tokenScopes"
                :items="[...AVAILABLE_SCOPES]"
              />
            </UFormField>
            <UFormField label="Expires at" hint="Optional">
              <UInput v-model="tokenExpiresAt" type="datetime-local" />
            </UFormField>
            <UButton
              block
              icon="i-lucide-plus"
              :loading="creatingToken"
              :disabled="!tokenName.trim()"
              @click="createToken"
            >
              Create token
            </UButton>
          </div>

          <UAlert
            v-if="revealedToken"
            title="Copy this token now"
            description="The full token is shown only once and cannot be retrieved later."
            color="warning"
            variant="subtle"
          >
            <template #actions>
              <TokenReveal :token="revealedToken.token" />
            </template>
          </UAlert>

          <div>
            <div
              class="mb-2 text-sm font-semibold text-(--ui-text-highlighted)"
            >
              Active tokens
            </div>
            <div v-if="visibleTokens.length" class="space-y-2">
              <div
                v-for="token in visibleTokens"
                :key="token.token_id"
                class="flex items-center justify-between gap-3 rounded-lg border border-(--ui-border) px-3 py-2.5"
              >
                <div class="min-w-0">
                  <div
                    class="truncate text-sm font-medium text-(--ui-text-highlighted)"
                  >
                    {{ token.token_name }}
                  </div>
                  <div class="mt-0.5 text-xs text-(--ui-text-muted)">
                    Created {{ formatDateTime(token.created_at) }}
                    <template v-if="token.expires_at">
                      · expires {{ formatDateTime(token.expires_at) }}
                    </template>
                  </div>
                  <div class="text-xs text-(--ui-text-dimmed)">
                    {{ token.scopes.join(', ') || 'no scopes' }}
                  </div>
                </div>
                <UButton
                  icon="i-lucide-trash-2"
                  variant="ghost"
                  color="error"
                  size="sm"
                  :aria-label="`Revoke ${token.token_name}`"
                  @click="
                    () => {
                      revokeTarget = token
                    }
                  "
                />
              </div>
            </div>
            <p v-else class="text-sm text-(--ui-text-muted)">
              No tokens for this application yet.
            </p>
          </div>
        </div>
      </template>
    </UDrawer>

    <!-- Revoke token confirm -->
    <ConfirmModal
      v-model:open="revokeModalOpen"
      title="Revoke token"
      :description="
        revokeTarget
          ? `Clients using ${revokeTarget.token_name} will lose access immediately.`
          : undefined
      "
      confirm-label="Revoke"
      confirm-color="error"
      :loading="revoking"
      @confirm="confirmRevokeToken"
    />
  </div>
</template>
