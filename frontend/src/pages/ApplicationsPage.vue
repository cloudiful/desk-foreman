<script setup lang="ts">
import Button from 'primevue/button'
import Column from 'primevue/column'
import DataTable from 'primevue/datatable'
import Dialog from 'primevue/dialog'
import InputSwitch from 'primevue/inputswitch'
import InputNumber from 'primevue/inputnumber'
import InputText from 'primevue/inputtext'
import Select from 'primevue/select'
import { computed, onMounted, ref } from 'vue'
import {
  createAdminApplication,
  createAdminApplicationToken,
  deleteAdminApplicationToken,
  listAdminApplicationTokens,
  listAdminApplications,
  updateAdminApplication,
} from '../api/users'
import type {
  ApplicationResponse,
  ApplicationTokenResponse,
  CreateApplicationTokenResponse,
} from '../generated/openapi/types.gen'

type EditableApplication = ApplicationResponse
type TokenModalApplication = Pick<
  ApplicationResponse,
  'application_id' | 'name'
>

const rows = ref<EditableApplication[]>([])
const allTokens = ref<ApplicationTokenResponse[]>([])
const loading = ref(false)
const error = ref('')
const dialogVisible = ref(false)
const tokenDialogVisible = ref(false)
const tokenCreateName = ref('')
const tokenScopes = ref(
  'workspace.read, workspace.search, workspace.shell, workspace.patch',
)
const tokenExpiresAt = ref('')
const tokenCreateLoading = ref(false)
const tokenBusyId = ref<number | null>(null)
const tokenOwner = ref<TokenModalApplication | null>(null)
const revealedToken = ref<CreateApplicationTokenResponse | null>(null)
const editing = ref<EditableApplication | null>(null)

const visibleTokens = computed(() => {
  if (!tokenOwner.value) return []
  return allTokens.value.filter(
    (token) => token.application_id === tokenOwner.value?.application_id,
  )
})

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    const [applications, tokens] = await Promise.all([
      listAdminApplications(),
      listAdminApplicationTokens(),
    ])
    rows.value = applications
    allTokens.value = tokens
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to load applications'
  } finally {
    loading.value = false
  }
}

function startCreate(): void {
  editing.value = {
    application_id: 0,
    name: '',
    is_active: true,
    workspace_template: null,
    default_shell: null,
    default_scopes: [
      'workspace.read',
      'workspace.search',
      'workspace.shell',
      'workspace.patch',
    ],
    max_timeout_ms: null,
    max_output_bytes: null,
    max_file_bytes: null,
    max_sessions: null,
    network_enabled: true,
    approval_mode: 'inherit',
    approval_endpoint: null,
    approval_model: null,
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
  }
  dialogVisible.value = true
}

function startEdit(row: EditableApplication): void {
  editing.value = { ...row }
  dialogVisible.value = true
}

async function saveApplication(): Promise<void> {
  if (!editing.value) return
  error.value = ''
  try {
    if (!editing.value.name.trim()) {
      error.value = 'Name is required'
      return
    }
    if (editing.value.application_id === 0) {
      await createAdminApplication({
        name: editing.value.name.trim(),
        workspace_template: editing.value.workspace_template?.trim() || null,
        default_shell: editing.value.default_shell?.trim() || null,
        default_scopes: editing.value.default_scopes,
        max_timeout_ms: editing.value.max_timeout_ms,
        max_output_bytes: editing.value.max_output_bytes,
        max_file_bytes: editing.value.max_file_bytes,
        max_sessions: editing.value.max_sessions,
        network_enabled: editing.value.network_enabled,
        approval_mode: editing.value.approval_mode,
        approval_endpoint: editing.value.approval_endpoint?.trim() || null,
        approval_model: editing.value.approval_model?.trim() || null,
      })
    } else {
      await updateAdminApplication(editing.value.application_id, {
        name: editing.value.name.trim(),
        is_active: editing.value.is_active,
        workspace_template: editing.value.workspace_template?.trim() || null,
        default_shell: editing.value.default_shell?.trim() || null,
        default_scopes: editing.value.default_scopes,
        max_timeout_ms: editing.value.max_timeout_ms,
        max_output_bytes: editing.value.max_output_bytes,
        max_file_bytes: editing.value.max_file_bytes,
        max_sessions: editing.value.max_sessions,
        network_enabled: editing.value.network_enabled,
        approval_mode: editing.value.approval_mode,
        approval_endpoint: editing.value.approval_endpoint?.trim() || null,
        approval_model: editing.value.approval_model?.trim() || null,
      })
    }
    dialogVisible.value = false
    editing.value = null
    await load()
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to save application'
  }
}

function openTokenDialog(application: EditableApplication): void {
  tokenOwner.value = {
    application_id: application.application_id,
    name: application.name,
  }
  tokenCreateName.value = ''
  tokenScopes.value =
    'workspace.read, workspace.search, workspace.shell, workspace.patch'
  tokenExpiresAt.value = ''
  revealedToken.value = null
  tokenDialogVisible.value = true
}

async function createToken(): Promise<void> {
  if (!tokenOwner.value) return
  tokenCreateLoading.value = true
  error.value = ''
  try {
    revealedToken.value = await createAdminApplicationToken({
      application_id: tokenOwner.value.application_id,
      token_name: tokenCreateName.value.trim(),
      scopes: tokenScopes.value
        .split(',')
        .map((scope) => scope.trim())
        .filter(Boolean),
      expires_at: tokenExpiresAt.value || null,
    })
    tokenCreateName.value = ''
    allTokens.value = await listAdminApplicationTokens()
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to create application token'
  } finally {
    tokenCreateLoading.value = false
  }
}

async function revokeToken(token: ApplicationTokenResponse): Promise<void> {
  tokenBusyId.value = token.token_id
  error.value = ''
  try {
    await deleteAdminApplicationToken(token.token_id)
    allTokens.value = await listAdminApplicationTokens()
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to revoke application token'
  } finally {
    tokenBusyId.value = null
  }
}

onMounted(() => {
  void load()
})
</script>

<template>
  <section class="space-y-4">
    <div class="app-shell-panel rounded-[2rem] p-5">
      <div
        class="flex flex-col gap-3 md:flex-row md:items-center md:justify-between"
      >
        <div>
          <div class="text-xs uppercase tracking-[0.25em] text-[var(--muted)]">
            Admin
          </div>
          <h2 class="mt-2 text-2xl font-semibold">Applications</h2>
        </div>
        <Button label="Create application" @click="startCreate" />
      </div>
      <p v-if="error" class="mt-3 text-sm text-red-700">{{ error }}</p>
    </div>

    <div class="app-shell-panel overflow-hidden rounded-[2rem] p-3">
      <DataTable :value="rows" :loading="loading" striped-rows>
        <Column field="name" header="Name" />
        <Column field="workspace_template" header="Template" />
        <Column field="default_shell" header="Default shell" />
        <Column header="Active">
          <template #body="{ data }">
            {{ data.is_active ? 'Yes' : 'No' }}
          </template>
        </Column>
        <Column header="Actions">
          <template #body="{ data }">
            <div class="flex gap-2">
              <Button
                label="Edit"
                size="small"
                severity="secondary"
                @click="startEdit(data)"
              />
              <Button
                label="App tokens"
                size="small"
                severity="contrast"
                @click="openTokenDialog(data)"
              />
            </div>
          </template>
        </Column>
      </DataTable>
    </div>

    <Dialog
      v-model:visible="dialogVisible"
      modal
      :style="{ width: '34rem' }"
      header="Application"
    >
      <div v-if="editing" class="space-y-4">
        <div class="space-y-2">
          <label class="block text-sm font-medium">Name</label>
          <InputText v-model="editing.name" class="w-full" />
        </div>
        <div class="space-y-2">
          <label class="block text-sm font-medium">Workspace template</label>
          <InputText v-model="editing.workspace_template" class="w-full" />
        </div>
        <div class="space-y-2">
          <label class="block text-sm font-medium">Default shell</label>
          <InputText v-model="editing.default_shell" class="w-full" />
        </div>
        <div class="space-y-2">
          <label class="block text-sm font-medium">Default scopes</label>
          <InputText
            :model-value="editing.default_scopes.join(', ')"
            class="w-full"
            @update:model-value="
              editing.default_scopes = String($event)
                .split(',')
                .map((scope) => scope.trim())
                .filter(Boolean)
            "
          />
        </div>
        <div class="grid gap-3 sm:grid-cols-2">
          <InputNumber
            v-model="editing.max_timeout_ms"
            input-class="w-full"
            placeholder="Max timeout (ms)"
          />
          <InputNumber
            v-model="editing.max_output_bytes"
            input-class="w-full"
            placeholder="Max output bytes"
          />
          <InputNumber
            v-model="editing.max_file_bytes"
            input-class="w-full"
            placeholder="Max file bytes"
          />
          <InputNumber
            v-model="editing.max_sessions"
            input-class="w-full"
            placeholder="Max sessions"
          />
        </div>
        <div class="space-y-3 rounded-2xl bg-black/4 p-4">
          <div class="text-sm font-medium">Approval reviewer</div>
          <Select
            v-model="editing.approval_mode"
            :options="[
              { label: 'Inherit global', value: 'inherit' },
              { label: 'Disabled', value: 'disabled' },
              { label: 'Use application reviewer', value: 'enabled' },
            ]"
            option-label="label"
            option-value="value"
            class="w-full"
          />
          <InputText
            v-if="editing.approval_mode === 'enabled'"
            v-model="editing.approval_endpoint"
            class="w-full"
            placeholder="Responses API base URL"
          />
          <InputText
            v-if="editing.approval_mode === 'enabled'"
            v-model="editing.approval_model"
            class="w-full"
            placeholder="Model"
          />
        </div>
        <div
          v-if="editing.application_id !== 0"
          class="flex items-center justify-between rounded-2xl bg-black/4 px-4 py-3"
        >
          <span>Active</span>
          <InputSwitch v-model="editing.is_active" />
        </div>
        <div
          class="flex items-center justify-between rounded-2xl bg-black/4 px-4 py-3"
        >
          <span>Network enabled</span>
          <InputSwitch v-model="editing.network_enabled" />
        </div>
      </div>
      <template #footer>
        <Button
          label="Cancel"
          severity="secondary"
          text
          @click="dialogVisible = false"
        />
        <Button label="Save" @click="saveApplication" />
      </template>
    </Dialog>

    <Dialog
      v-model:visible="tokenDialogVisible"
      modal
      :style="{ width: '42rem' }"
      :header="`Application tokens · ${tokenOwner?.name || ''}`"
    >
      <div class="space-y-4">
        <div class="flex gap-3">
          <InputText
            v-model="tokenCreateName"
            class="flex-1"
            placeholder="Token name"
          />
          <Button
            label="Create"
            :loading="tokenCreateLoading"
            :disabled="!tokenCreateName.trim()"
            @click="createToken"
          />
        </div>
        <div class="grid gap-3 md:grid-cols-2">
          <InputText
            v-model="tokenScopes"
            placeholder="Scopes, comma separated"
          />
          <InputText
            v-model="tokenExpiresAt"
            type="datetime-local"
            placeholder="Expires at"
          />
        </div>
        <div
          v-if="revealedToken"
          class="rounded-2xl bg-amber-50 p-4 text-sm text-amber-900"
        >
          <div class="font-medium">Copy this token now</div>
          <div class="mt-2 break-all font-mono">{{ revealedToken.token }}</div>
        </div>
        <DataTable :value="visibleTokens" striped-rows>
          <Column field="token_name" header="Token name" />
          <Column field="created_at" header="Created at" />
          <Column field="last_used_at" header="Last used" />
          <Column field="expires_at" header="Expires" />
          <Column header="Scopes">
            <template #body="{ data }">{{ data.scopes.join(', ') }}</template>
          </Column>
          <Column header="Actions">
            <template #body="{ data }">
              <Button
                label="Revoke"
                size="small"
                severity="danger"
                :loading="tokenBusyId === data.token_id"
                @click="revokeToken(data)"
              />
            </template>
          </Column>
        </DataTable>
      </div>
    </Dialog>
  </section>
</template>
