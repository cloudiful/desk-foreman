<script setup lang="ts">
import Button from 'primevue/button'
import Column from 'primevue/column'
import DataTable from 'primevue/datatable'
import Dialog from 'primevue/dialog'
import InputSwitch from 'primevue/inputswitch'
import InputText from 'primevue/inputtext'
import Password from 'primevue/password'
import Textarea from 'primevue/textarea'
import { computed, onMounted, ref } from 'vue'
import {
  createAdminMcpToken,
  createAdminUser,
  deleteAdminMcpToken,
  deactivateAdminUser,
  listAdminMcpTokens,
  listAdminUsers,
  resetAdminUserPassword,
  updateAdminUser,
} from '../api/users'
import type {
  CreateMcpTokenResponse,
  McpTokenResponse,
  UserResponse,
} from '../generated/openapi/types.gen'

type EditableUser = UserResponse & {
  password?: string
}

type TokenModalUser = Pick<
  UserResponse,
  'user_id' | 'display_name' | 'login_name'
>

const rows = ref<EditableUser[]>([])
const allTokens = ref<McpTokenResponse[]>([])
const total = ref(0)
const loading = ref(false)
const error = ref('')
const dialogVisible = ref(false)
const resetVisible = ref(false)
const tokenDialogVisible = ref(false)
const tokenCreateName = ref('')
const tokenCreateLoading = ref(false)
const tokenBusyId = ref<number | null>(null)
const tokenOwner = ref<TokenModalUser | null>(null)
const revealedToken = ref<CreateMcpTokenResponse | null>(null)
const editing = ref<EditableUser | null>(null)
const resetTarget = ref<EditableUser | null>(null)
const newPassword = ref('')

const visibleTokens = computed(() => {
  if (!tokenOwner.value) return []
  return allTokens.value.filter(
    (token) => token.user_id === tokenOwner.value?.user_id,
  )
})

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    const [users, tokens] = await Promise.all([
      listAdminUsers({
        limit: 20,
        offset: 0,
        sort_by: 'updated_at',
        sort_dir: 'desc',
      }),
      listAdminMcpTokens(),
    ])
    rows.value = users.items
    total.value = users.total
    allTokens.value = tokens
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Failed to load users'
  } finally {
    loading.value = false
  }
}

function startCreate(): void {
  editing.value = {
    user_id: 0,
    login_name: '',
    display_name: '',
    email: '',
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC',
    workspace_root: '',
    is_admin: false,
    is_active: true,
    last_login_at: null,
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    password: '',
  }
  dialogVisible.value = true
}

function startEdit(row: EditableUser): void {
  editing.value = { ...row }
  dialogVisible.value = true
}

function validateCreateUserForm(user: EditableUser): string | null {
  const missingFields = [
    !user.login_name.trim() ? 'login name' : null,
    !user.password ? 'password' : null,
    !user.display_name.trim() ? 'display name' : null,
    !user.email.trim() ? 'email' : null,
  ].filter((field): field is string => field !== null)

  if (!missingFields.length) return null
  return `Missing required fields: ${missingFields.join(', ')}`
}

async function saveUser(): Promise<void> {
  if (!editing.value) return
  error.value = ''
  try {
    if (editing.value.user_id === 0) {
      const validationError = validateCreateUserForm(editing.value)
      if (validationError) {
        error.value = validationError
        return
      }
      await createAdminUser({
        login_name: editing.value.login_name.trim(),
        password: editing.value.password || '',
        display_name: editing.value.display_name.trim(),
        email: editing.value.email.trim(),
        timezone: editing.value.timezone.trim(),
        workspace_root: undefined,
        is_admin: editing.value.is_admin,
      })
    } else {
      await updateAdminUser(editing.value.user_id, {
        display_name: editing.value.display_name.trim(),
        email: editing.value.email.trim(),
        timezone: editing.value.timezone.trim(),
        is_admin: editing.value.is_admin,
        is_active: editing.value.is_active,
      })
    }
    dialogVisible.value = false
    editing.value = null
    await load()
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Failed to save user'
  }
}

async function deactivate(row: EditableUser): Promise<void> {
  try {
    await deactivateAdminUser(row.user_id)
    await load()
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to deactivate user'
  }
}

async function submitReset(): Promise<void> {
  if (!resetTarget.value) return
  try {
    await resetAdminUserPassword(resetTarget.value.user_id, {
      password: newPassword.value,
    })
    resetVisible.value = false
    newPassword.value = ''
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to reset password'
  }
}

function openResetDialog(user: EditableUser): void {
  resetTarget.value = user
  resetVisible.value = true
}

function openTokenDialog(user: EditableUser): void {
  tokenOwner.value = {
    user_id: user.user_id,
    display_name: user.display_name,
    login_name: user.login_name,
  }
  tokenCreateName.value = ''
  revealedToken.value = null
  tokenDialogVisible.value = true
}

async function createToken(): Promise<void> {
  if (!tokenOwner.value) return
  tokenCreateLoading.value = true
  error.value = ''
  try {
    revealedToken.value = await createAdminMcpToken({
      user_id: tokenOwner.value.user_id,
      token_name: tokenCreateName.value.trim(),
    })
    tokenCreateName.value = ''
    allTokens.value = await listAdminMcpTokens()
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to create MCP token'
  } finally {
    tokenCreateLoading.value = false
  }
}

async function revokeToken(token: McpTokenResponse): Promise<void> {
  tokenBusyId.value = token.token_id
  error.value = ''
  try {
    await deleteAdminMcpToken(token.token_id)
    allTokens.value = await listAdminMcpTokens()
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to revoke MCP token'
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
          <h2 class="mt-2 text-2xl font-semibold">Users</h2>
        </div>
        <Button label="Create user" @click="startCreate" />
      </div>
      <p v-if="error" class="mt-3 text-sm text-red-700">{{ error }}</p>
    </div>

    <div class="app-shell-panel overflow-hidden rounded-[2rem] p-3">
      <DataTable :value="rows" :loading="loading" striped-rows>
        <Column field="login_name" header="Login" />
        <Column field="display_name" header="Display name" />
        <Column field="email" header="Email" />
        <Column field="timezone" header="Timezone" />
        <Column field="workspace_root" header="Workspace" />
        <Column header="Admin">
          <template #body="{ data }">
            {{ data.is_admin ? 'Yes' : 'No' }}
          </template>
        </Column>
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
                label="MCP tokens"
                size="small"
                severity="contrast"
                @click="openTokenDialog(data)"
              />
              <Button
                label="Reset password"
                size="small"
                severity="help"
                @click="openResetDialog(data)"
              />
              <Button
                label="Deactivate"
                size="small"
                severity="danger"
                :disabled="!data.is_active"
                @click="deactivate(data)"
              />
            </div>
          </template>
        </Column>
      </DataTable>
      <div class="mt-4 text-sm text-[var(--muted)]">
        Total users: {{ total }}
      </div>
    </div>

    <Dialog
      v-model:visible="dialogVisible"
      modal
      :style="{ width: '34rem' }"
      header="User"
    >
      <div v-if="editing" class="space-y-4">
        <div class="space-y-2">
          <label class="block text-sm font-medium">Login name</label>
          <InputText
            v-model="editing.login_name"
            class="w-full"
            :disabled="editing.user_id !== 0"
          />
        </div>
        <div v-if="editing.user_id === 0" class="space-y-2">
          <label class="block text-sm font-medium">Password</label>
          <Password
            v-model="editing.password"
            class="w-full"
            fluid
            :feedback="false"
            toggle-mask
          />
        </div>
        <div class="space-y-2">
          <label class="block text-sm font-medium">Display name</label>
          <InputText v-model="editing.display_name" class="w-full" />
        </div>
        <div class="space-y-2">
          <label class="block text-sm font-medium">Email</label>
          <InputText v-model="editing.email" class="w-full" />
        </div>
        <div class="space-y-2">
          <label class="block text-sm font-medium">Timezone</label>
          <InputText v-model="editing.timezone" class="w-full" />
        </div>
        <div class="space-y-2">
          <label class="block text-sm font-medium">Workspace</label>
          <InputText
            :model-value="
              editing.workspace_root ||
              'Auto-assigned to /workspace/users/<user_id>'
            "
            class="w-full"
            disabled
          />
        </div>
        <div
          class="flex items-center justify-between rounded-2xl bg-black/4 px-4 py-3"
        >
          <span>Administrator</span>
          <InputSwitch v-model="editing.is_admin" />
        </div>
        <div
          v-if="editing.user_id !== 0"
          class="flex items-center justify-between rounded-2xl bg-black/4 px-4 py-3"
        >
          <span>Active</span>
          <InputSwitch v-model="editing.is_active" />
        </div>
      </div>
      <template #footer>
        <Button
          label="Cancel"
          severity="secondary"
          text
          @click="dialogVisible = false"
        />
        <Button label="Save" @click="saveUser" />
      </template>
    </Dialog>

    <Dialog
      v-model:visible="resetVisible"
      modal
      :style="{ width: '28rem' }"
      header="Reset password"
    >
      <div class="space-y-3">
        <p class="text-sm text-[var(--muted)]">
          Set a new password for
          {{ resetTarget?.display_name || resetTarget?.login_name }}.
        </p>
        <Password
          v-model="newPassword"
          class="w-full"
          fluid
          :feedback="false"
          toggle-mask
        />
      </div>
      <template #footer>
        <Button
          label="Cancel"
          severity="secondary"
          text
          @click="resetVisible = false"
        />
        <Button label="Reset password" @click="submitReset" />
      </template>
    </Dialog>

    <Dialog
      v-model:visible="tokenDialogVisible"
      modal
      :style="{ width: '42rem' }"
      header="MCP tokens"
    >
      <div class="space-y-4">
        <div class="rounded-2xl bg-black/4 px-4 py-3 text-sm">
          <div class="font-medium">
            {{ tokenOwner?.display_name || tokenOwner?.login_name }}
          </div>
          <div class="mt-1 text-[var(--muted)]">
            {{ tokenOwner?.login_name }}
          </div>
        </div>

        <div class="grid gap-3 md:grid-cols-[1fr_auto]">
          <InputText v-model="tokenCreateName" placeholder="Token name" />
          <Button
            label="Create token"
            :loading="tokenCreateLoading"
            :disabled="!tokenOwner || !tokenCreateName.trim()"
            @click="createToken"
          />
        </div>

        <div
          v-if="revealedToken"
          class="space-y-2 rounded-2xl border border-amber-300 bg-amber-50 px-4 py-3"
        >
          <div class="text-sm font-medium text-amber-900">New token</div>
          <p class="text-sm text-amber-900">This token is shown only once.</p>
          <Textarea
            :model-value="revealedToken.token"
            auto-resize
            readonly
            rows="3"
            class="w-full font-mono"
          />
        </div>

        <div class="space-y-3">
          <div
            v-for="token in visibleTokens"
            :key="token.token_id"
            class="flex flex-col gap-3 rounded-2xl border border-black/8 px-4 py-3 md:flex-row md:items-center md:justify-between"
          >
            <div class="min-w-0">
              <div class="font-medium">{{ token.token_name }}</div>
              <div class="mt-1 text-sm text-[var(--muted)]">
                Created {{ new Date(token.created_at).toLocaleString() }}
              </div>
              <div class="text-sm text-[var(--muted)]">
                Last used
                {{
                  token.last_used_at
                    ? new Date(token.last_used_at).toLocaleString()
                    : 'Never'
                }}
              </div>
            </div>
            <Button
              label="Revoke"
              size="small"
              severity="danger"
              :loading="tokenBusyId === token.token_id"
              @click="revokeToken(token)"
            />
          </div>
          <p v-if="!visibleTokens.length" class="text-sm text-[var(--muted)]">
            No active MCP tokens for this user.
          </p>
        </div>
      </div>
      <template #footer>
        <Button
          label="Close"
          severity="secondary"
          text
          @click="tokenDialogVisible = false"
        />
      </template>
    </Dialog>
  </section>
</template>
