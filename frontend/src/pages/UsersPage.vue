<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  createAdminMcpToken,
  createAdminUser,
  deactivateAdminUser,
  deleteAdminMcpToken,
  listAdminMcpTokens,
  listAdminUsers,
  resetAdminUserPassword,
  updateAdminUser,
} from '../api/users'
import { useNotify } from '../composables/useNotify'
import { formatDateTime, formatRelative, initials } from '../utils/format'
import type {
  CreateMcpTokenResponse,
  McpTokenResponse,
  UserResponse,
} from '../generated/openapi/types.gen'

const PAGE_SIZE = 100
const AVAILABLE_SCOPES = [
  'workspace.read',
  'workspace.search',
  'workspace.shell',
  'workspace.patch',
] as const
type AvailableScope = (typeof AVAILABLE_SCOPES)[number]

const { success, error: notifyError } = useNotify()
const { t } = useI18n()

const rows = ref<UserResponse[]>([])
const loading = ref(false)
const error = ref('')
const total = ref(0)
const page = ref(1)

const search = ref('')
const roleFilter = ref<'all' | 'admin' | 'user'>('all')
const statusFilter = ref<'all' | 'active' | 'inactive'>('all')
const sort = ref<{ key: string; dir: 'asc' | 'desc' } | null>({
  key: 'updated_at',
  dir: 'desc',
})
let loadSequence = 0

const pageCount = computed(() =>
  total.value > 0 ? Math.max(1, Math.ceil(total.value / PAGE_SIZE)) : 1,
)

async function load(): Promise<void> {
  const sequence = ++loadSequence
  loading.value = true
  error.value = ''
  try {
    const result = await listAdminUsers({
      limit: PAGE_SIZE,
      offset: (page.value - 1) * PAGE_SIZE,
      sort_by: sort.value?.key,
      sort_dir: sort.value?.dir,
      search: search.value.trim() || undefined,
      is_admin:
        roleFilter.value === 'all' ? undefined : roleFilter.value === 'admin',
      is_active:
        statusFilter.value === 'all'
          ? undefined
          : statusFilter.value === 'active',
    })
    if (sequence !== loadSequence) return
    rows.value = result.items
    total.value = result.total
  } catch (err) {
    if (sequence === loadSequence) {
      error.value = err instanceof Error ? err.message : t('users.errors.load')
    }
  } finally {
    if (sequence === loadSequence) loading.value = false
  }
}

function onPageChange(): void {
  void load()
}

function onSearchEnter(): void {
  page.value = 1
  void load()
}

watch([roleFilter, statusFilter], () => {
  page.value = 1
  void load()
})

function changeSort(value: { key: string; dir: 'asc' | 'desc' } | null): void {
  sort.value = value
  page.value = 1
  void load()
}

// ----- user create/edit -----
const editing = ref<{
  user_id: number
  login_name: string
  display_name: string
  email: string
  timezone: string
  workspace_root: string
  is_admin: boolean
  is_active: boolean
  password: string
} | null>(null)
const userDrawerOpen = ref(false)
const savingUser = ref(false)
const userFormError = ref('')

function timezones(): string[] {
  try {
    return Intl.supportedValuesOf('timeZone')
  } catch {
    return ['UTC', 'Asia/Shanghai', 'Europe/London', 'America/New_York']
  }
}

function startCreate(): void {
  userFormError.value = ''
  editing.value = {
    user_id: 0,
    login_name: '',
    display_name: '',
    email: '',
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC',
    workspace_root: '',
    is_admin: false,
    is_active: true,
    password: '',
  }
  userDrawerOpen.value = true
}

function startEdit(row: UserResponse): void {
  userFormError.value = ''
  editing.value = {
    user_id: row.user_id,
    login_name: row.login_name,
    display_name: row.display_name,
    email: row.email,
    timezone: row.timezone,
    workspace_root: row.workspace_root,
    is_admin: row.is_admin,
    is_active: row.is_active,
    password: '',
  }
  userDrawerOpen.value = true
}

function handleUserDrawerOpen(open: boolean): void {
  if (!open && !savingUser.value) {
    editing.value = null
    userFormError.value = ''
  }
}

function validateUser(): string | null {
  const form = editing.value
  if (!form) return null
  if (!form.login_name.trim()) return t('users.validation.loginNameRequired')
  if (!form.display_name.trim())
    return t('users.validation.displayNameRequired')
  if (!form.email.trim()) return t('users.validation.emailRequired')
  if (form.user_id === 0 && !form.password)
    return t('users.validation.passwordRequired')
  if (form.user_id === 0 && form.password.length < 8)
    return t('users.validation.passwordLength')
  return null
}

async function saveUser(): Promise<void> {
  if (!editing.value || savingUser.value) return
  userFormError.value = validateUser() ?? ''
  if (userFormError.value) return
  savingUser.value = true
  try {
    if (editing.value.user_id === 0) {
      await createAdminUser({
        login_name: editing.value.login_name.trim(),
        password: editing.value.password,
        display_name: editing.value.display_name.trim(),
        email: editing.value.email.trim(),
        timezone: editing.value.timezone.trim(),
        is_admin: editing.value.is_admin,
        workspace_root: null,
      })
      success(
        t('users.notifications.userCreated'),
        t('users.notifications.userCreatedDescription', {
          login: editing.value.login_name,
        }),
      )
    } else {
      await updateAdminUser(editing.value.user_id, {
        display_name: editing.value.display_name.trim(),
        email: editing.value.email.trim(),
        timezone: editing.value.timezone.trim(),
        is_admin: editing.value.is_admin,
        is_active: editing.value.is_active,
      })
      success(t('users.notifications.userUpdated'), editing.value.login_name)
    }
    userDrawerOpen.value = false
    editing.value = null
    await load()
  } catch (err) {
    userFormError.value =
      err instanceof Error ? err.message : t('users.errors.save')
  } finally {
    savingUser.value = false
  }
}

// ----- deactivate -----
const deactivateTarget = ref<UserResponse | null>(null)
const deactivating = ref(false)

const deactivateModalOpen = computed<boolean>({
  get: () => Boolean(deactivateTarget.value),
  set: (open) => {
    if (!open) deactivateTarget.value = null
  },
})

async function confirmDeactivate(): Promise<void> {
  if (!deactivateTarget.value || deactivating.value) return
  deactivating.value = true
  try {
    await deactivateAdminUser(deactivateTarget.value.user_id)
    success(
      t('users.notifications.userDeactivated'),
      deactivateTarget.value.login_name,
    )
    deactivateTarget.value = null
    await load()
  } catch (err) {
    notifyError(
      t('users.errors.deactivate'),
      err instanceof Error ? err.message : undefined,
    )
  } finally {
    deactivating.value = false
  }
}

async function activateUser(row: UserResponse): Promise<void> {
  try {
    await updateAdminUser(row.user_id, {
      display_name: row.display_name,
      email: row.email,
      timezone: row.timezone,
      is_admin: row.is_admin,
      is_active: true,
    })
    success(t('users.notifications.userActivated'), row.login_name)
    await load()
  } catch (err) {
    notifyError(
      t('users.errors.activate'),
      err instanceof Error ? err.message : undefined,
    )
  }
}

// ----- reset password -----
const resetTarget = ref<UserResponse | null>(null)
const newPassword = ref('')
const resetting = ref(false)
const resetError = ref('')

const resetModalOpen = computed<boolean>({
  get: () => Boolean(resetTarget.value),
  set: (open) => {
    if (!open) resetTarget.value = null
  },
})

function openReset(row: UserResponse): void {
  resetError.value = ''
  newPassword.value = ''
  resetTarget.value = row
}

async function confirmReset(): Promise<void> {
  if (!resetTarget.value || resetting.value) return
  if (newPassword.value.length < 8) {
    resetError.value = t('users.validation.passwordLength')
    return
  }
  resetting.value = true
  resetError.value = ''
  try {
    await resetAdminUserPassword(resetTarget.value.user_id, {
      password: newPassword.value,
    })
    success(
      t('users.notifications.passwordReset'),
      resetTarget.value.login_name,
    )
    resetTarget.value = null
    newPassword.value = ''
  } catch (err) {
    resetError.value =
      err instanceof Error ? err.message : t('users.errors.reset')
  } finally {
    resetting.value = false
  }
}

// ----- MCP tokens -----
const tokenOwner = ref<UserResponse | null>(null)
const allTokens = ref<McpTokenResponse[]>([])
const tokenTotal = ref(0)
const tokenPage = ref(1)
const tokenDrawerOpen = ref(false)
const tokenName = ref('')
const tokenScopes = ref<AvailableScope[]>([...AVAILABLE_SCOPES])
const tokenExpiresAt = ref('')
const creatingToken = ref(false)
const revealedToken = ref<CreateMcpTokenResponse | null>(null)
const revokeTarget = ref<McpTokenResponse | null>(null)
const revoking = ref(false)
let tokenLoadSequence = 0

const tokenTotalPages = computed(() =>
  tokenTotal.value > 0 ? Math.ceil(tokenTotal.value / PAGE_SIZE) : 1,
)

const visibleTokens = computed(() =>
  tokenOwner.value
    ? allTokens.value.filter((t) => t.user_id === tokenOwner.value?.user_id)
    : [],
)

const revokeModalOpen = computed<boolean>({
  get: () => Boolean(revokeTarget.value),
  set: (open) => {
    if (!open) revokeTarget.value = null
  },
})

async function openTokenDrawer(row: UserResponse): Promise<void> {
  tokenOwner.value = row
  tokenName.value = ''
  tokenScopes.value = [...AVAILABLE_SCOPES]
  tokenExpiresAt.value = ''
  revealedToken.value = null
  tokenPage.value = 1
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
  if (!tokenOwner.value) return
  const sequence = ++tokenLoadSequence
  try {
    const result = await listAdminMcpTokens({
      user_id: tokenOwner.value.user_id,
      limit: PAGE_SIZE,
      offset: (tokenPage.value - 1) * PAGE_SIZE,
    })
    if (sequence === tokenLoadSequence) {
      allTokens.value = result.items
      tokenTotal.value = result.total
    }
  } catch (err) {
    notifyError(
      t('users.errors.loadTokens'),
      err instanceof Error ? err.message : undefined,
    )
  }
}

function onTokenPageChange(): void {
  void loadTokens()
}

async function createToken(): Promise<void> {
  if (!tokenOwner.value || !tokenName.value.trim() || creatingToken.value)
    return
  creatingToken.value = true
  try {
    revealedToken.value = await createAdminMcpToken({
      user_id: tokenOwner.value.user_id,
      token_name: tokenName.value.trim(),
      scopes: tokenScopes.value,
      expires_at: tokenExpiresAt.value
        ? new Date(tokenExpiresAt.value).toISOString()
        : null,
    })
    tokenName.value = ''
    await loadTokens()
    success(t('shared.token.createdNotification'))
  } catch (err) {
    notifyError(
      t('users.errors.createToken'),
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
    await deleteAdminMcpToken(revokeTarget.value.token_id)
    success(
      t('shared.token.revokedNotification'),
      revokeTarget.value.token_name,
    )
    revokeTarget.value = null
    await loadTokens()
  } catch (err) {
    notifyError(
      t('users.errors.revokeToken'),
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
    <PageHeader>
      <template #actions>
        <UButton
          icon="i-lucide-refresh-cw"
          variant="outline"
          color="neutral"
          :loading="loading"
          :aria-label="t('users.actions.refresh')"
          @click="load"
        />
        <UButton icon="i-lucide-user-plus" @click="startCreate">
          {{ t('users.actions.create') }}
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
          :placeholder="t('users.searchPlaceholder')"
          leading-icon="i-lucide-search"
          class="md:max-w-xs"
          @keyup.enter="onSearchEnter"
        />
        <div class="flex flex-wrap items-center gap-2">
          <USelect
            v-model="roleFilter"
            :items="[
              { label: t('users.filters.allRoles'), value: 'all' },
              { label: t('users.filters.admins'), value: 'admin' },
              { label: t('users.filters.users'), value: 'user' },
            ]"
            class="w-36"
          />
          <USelect
            v-model="statusFilter"
            :items="[
              { label: t('users.filters.allStatuses'), value: 'all' },
              { label: t('users.filters.active'), value: 'active' },
              { label: t('users.filters.inactive'), value: 'inactive' },
            ]"
            class="w-36"
          />
          <span class="ml-auto text-sm text-(--ui-text-muted)">
            {{ t('users.count', { shown: rows.length, total }) }}
          </span>
        </div>
      </div>

      <ErrorAlert v-if="error" :error="error" class="m-4" @retry="load" />

      <DataTable
        :rows="rows"
        :columns="[
          { key: 'login_name', label: t('users.table.user'), sortable: true },
          { key: 'role', label: t('users.table.role') },
          { key: 'status', label: t('users.table.status') },
          {
            key: 'last_login_at',
            label: t('users.table.lastLogin'),
            sortable: true,
          },
          {
            key: 'updated_at',
            label: t('users.table.updated'),
            sortable: true,
          },
          { key: 'actions', label: '', class: 'text-right' },
        ]"
        :loading="loading"
        :row-key="(row) => row.user_id as number"
        :sort="sort"
        :empty-title="
          search ? t('users.empty.matchingTitle') : t('users.empty.title')
        "
        :empty-description="
          search
            ? t('users.empty.searchDescription')
            : t('users.empty.description')
        "
        @update:sort="changeSort"
      >
        <template #cell-login_name="{ row }">
          <div class="flex items-center gap-3">
            <UAvatar
              :alt="(row.display_name as string) ?? (row.login_name as string)"
              :text="
                initials(
                  (row.display_name as string) ?? (row.login_name as string),
                )
              "
              size="sm"
            />
            <div class="min-w-0">
              <div class="truncate font-medium text-(--ui-text-highlighted)">
                {{ row.display_name }}
              </div>
              <div class="truncate text-xs text-(--ui-text-muted)">
                @{{ row.login_name }}
              </div>
            </div>
          </div>
        </template>
        <template #cell-role="{ row }">
          <UBadge v-if="row.is_admin" color="primary" variant="soft" size="sm">
            {{ t('users.roles.admin') }}
          </UBadge>
          <span v-else class="text-sm text-(--ui-text-muted)">
            {{ t('users.roles.user') }}
          </span>
        </template>
        <template #cell-status="{ row }">
          <StatusBadge
            :status="(row.is_active as boolean) ? 'active' : 'inactive'"
          />
        </template>
        <template #cell-last_login_at="{ row }">
          <span
            class="whitespace-nowrap text-sm"
            :title="formatDateTime(row.last_login_at as string | null)"
          >
            {{ formatRelative(row.last_login_at as string | null) }}
          </span>
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
              :aria-label="t('users.actions.edit')"
              @click="startEdit(row as unknown as UserResponse)"
            />
            <UButton
              icon="i-lucide-key-round"
              variant="ghost"
              color="neutral"
              size="sm"
              :aria-label="t('users.actions.tokens')"
              @click="openTokenDrawer(row as unknown as UserResponse)"
            />
            <UDropdownMenu
              :items="[
                [
                  {
                    label: t('users.actions.resetPassword'),
                    icon: 'i-lucide-refresh-ccw',
                    onSelect: () => openReset(row as unknown as UserResponse),
                  },
                  {
                    label: (row.is_active as boolean)
                      ? t('users.actions.deactivate')
                      : t('users.actions.activate'),
                    icon: (row.is_active as boolean)
                      ? 'i-lucide-user-x'
                      : 'i-lucide-user-check',
                    color: 'error',
                    onSelect: () => {
                      if (row.is_active) {
                        deactivateTarget = row as unknown as UserResponse
                      } else {
                        void activateUser(row as unknown as UserResponse)
                      }
                    },
                  },
                ],
              ]"
            >
              <UButton
                icon="i-lucide-more-horizontal"
                variant="ghost"
                color="neutral"
                size="sm"
                :aria-label="t('users.actions.more')"
              />
            </UDropdownMenu>
          </div>
        </template>
      </DataTable>

      <div
        v-if="pageCount > 1"
        class="flex items-center justify-between border-t border-(--ui-border) px-4 py-3"
      >
        <span class="text-sm text-(--ui-text-muted)">
          {{ t('users.pagination', { page, totalPages: pageCount }) }}
        </span>
        <UPagination
          v-model:page="page"
          :total="total"
          :items-per-page="PAGE_SIZE"
          @update:page="onPageChange"
        />
      </div>
    </section>

    <!-- User create/edit drawer -->
    <UDrawer
      v-model:open="userDrawerOpen"
      :title="
        editing?.user_id === 0
          ? t('users.drawer.createTitle')
          : t('users.drawer.editTitle')
      "
      :dismissible="!savingUser"
      @update:open="handleUserDrawerOpen"
    >
      <template #body>
        <form
          v-if="editing"
          id="user-form"
          class="space-y-5"
          @submit.prevent="saveUser"
        >
          <UFormField :label="t('users.fields.loginName')">
            <UInput
              v-model="editing.login_name"
              name="login_name"
              autocomplete="off"
              :disabled="editing.user_id !== 0"
              :placeholder="t('users.placeholders.loginName')"
            />
          </UFormField>
          <UFormField
            v-if="editing.user_id === 0"
            :label="t('users.fields.password')"
            :hint="t('users.hints.password')"
          >
            <UInput
              v-model="editing.password"
              name="password"
              type="password"
              autocomplete="new-password"
              :placeholder="t('users.placeholders.password')"
            />
          </UFormField>
          <UFormField :label="t('users.fields.displayName')">
            <UInput
              v-model="editing.display_name"
              name="display_name"
              :placeholder="t('users.placeholders.displayName')"
            />
          </UFormField>
          <UFormField :label="t('users.fields.email')">
            <UInput
              v-model="editing.email"
              name="email"
              type="email"
              :placeholder="t('users.placeholders.email')"
            />
          </UFormField>
          <UFormField :label="t('users.fields.timezone')">
            <USelectMenu
              v-model="editing.timezone"
              :items="timezones().map((zone) => ({ label: zone, value: zone }))"
              value-key="value"
              searchable
              class="w-full"
            />
          </UFormField>
          <UFormField
            v-if="editing.user_id !== 0"
            :label="t('users.fields.workspaceRoot')"
            :hint="t('users.hints.workspaceRoot')"
          >
            <UInput
              :model-value="editing.workspace_root"
              readonly
              leading-icon="i-lucide-folder"
            />
          </UFormField>
          <div class="space-y-3 rounded-lg border border-(--ui-border) p-4">
            <div class="flex items-center justify-between">
              <div>
                <div class="text-sm font-medium text-(--ui-text-highlighted)">
                  {{ t('users.fields.administrator') }}
                </div>
                <div class="text-xs text-(--ui-text-muted)">
                  {{ t('users.hints.administrator') }}
                </div>
              </div>
              <USwitch v-model="editing.is_admin" />
            </div>
            <div
              v-if="editing.user_id !== 0"
              class="flex items-center justify-between"
            >
              <div>
                <div class="text-sm font-medium text-(--ui-text-highlighted)">
                  {{ t('users.fields.active') }}
                </div>
                <div class="text-xs text-(--ui-text-muted)">
                  {{ t('users.hints.active') }}
                </div>
              </div>
              <USwitch v-model="editing.is_active" />
            </div>
          </div>
          <UAlert
            v-if="userFormError"
            :title="userFormError"
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
            :disabled="savingUser"
            @click="
              () => {
                userDrawerOpen = false
              }
            "
          >
            {{ t('shared.confirm.cancel') }}
          </UButton>
          <UButton
            type="submit"
            form="user-form"
            :loading="savingUser"
            :disabled="savingUser"
          >
            {{
              editing?.user_id === 0
                ? t('users.actions.create')
                : t('users.actions.saveChanges')
            }}
          </UButton>
        </div>
      </template>
    </UDrawer>

    <!-- Reset password modal -->
    <ConfirmModal
      v-model:open="resetModalOpen"
      :title="t('users.confirmations.resetTitle')"
      :description="
        resetTarget
          ? t('users.confirmations.resetDescription', {
              displayName: resetTarget.display_name,
              login: resetTarget.login_name,
            })
          : undefined
      "
      :confirm-label="t('users.actions.resetPassword')"
      :loading="resetting"
      @confirm="confirmReset"
    >
      <div class="space-y-3">
        <UInput
          v-model="newPassword"
          type="password"
          :placeholder="t('users.placeholders.newPassword')"
          autocomplete="new-password"
        />
        <UAlert
          v-if="resetError"
          :title="resetError"
          color="error"
          variant="subtle"
        />
      </div>
    </ConfirmModal>

    <!-- Deactivate confirm modal -->
    <ConfirmModal
      v-model:open="deactivateModalOpen"
      :title="t('users.confirmations.deactivateTitle')"
      :description="
        deactivateTarget
          ? t('users.confirmations.deactivateDescription', {
              displayName: deactivateTarget.display_name,
            })
          : undefined
      "
      :confirm-label="t('users.actions.deactivate')"
      confirm-color="error"
      :loading="deactivating"
      @confirm="confirmDeactivate"
    />

    <!-- MCP tokens drawer -->
    <UDrawer
      v-model:open="tokenDrawerOpen"
      :title="t('users.tokens.title')"
      @update:open="handleTokenDrawerOpen"
    >
      <template #body>
        <div class="space-y-6">
          <div
            v-if="tokenOwner"
            class="flex items-center gap-3 rounded-lg border border-(--ui-border) p-3"
          >
            <UAvatar
              :alt="tokenOwner.display_name"
              :text="initials(tokenOwner.display_name)"
              size="sm"
            />
            <div>
              <div class="text-sm font-medium text-(--ui-text-highlighted)">
                {{ tokenOwner.display_name }}
              </div>
              <div class="text-xs text-(--ui-text-muted)">
                @{{ tokenOwner.login_name }}
              </div>
            </div>
          </div>

          <div class="space-y-4 rounded-lg border border-(--ui-border) p-4">
            <div class="text-sm font-semibold text-(--ui-text-highlighted)">
              {{ t('shared.token.createTitle') }}
            </div>
            <UFormField :label="t('shared.token.tokenName')">
              <UInput
                v-model="tokenName"
                :placeholder="t('users.placeholders.tokenName')"
                @keyup.enter="createToken"
              />
            </UFormField>
            <UFormField :label="t('shared.token.scopes')">
              <UCheckboxGroup
                v-model="tokenScopes"
                :items="[...AVAILABLE_SCOPES]"
              />
            </UFormField>
            <UFormField
              :label="t('shared.token.expiresAt')"
              :hint="t('shared.token.optional')"
            >
              <UInput v-model="tokenExpiresAt" type="datetime-local" />
            </UFormField>
            <UButton
              block
              icon="i-lucide-plus"
              :loading="creatingToken"
              :disabled="!tokenName.trim()"
              @click="createToken"
            >
              {{ t('shared.token.create') }}
            </UButton>
          </div>

          <UAlert
            v-if="revealedToken"
            :title="t('shared.token.copyTitle')"
            :description="t('shared.token.copyDescription')"
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
              {{ t('shared.token.active') }}
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
                    {{
                      t('shared.token.created', {
                        time: formatDateTime(token.created_at),
                      })
                    }}
                    <template v-if="token.expires_at">
                      ·
                      {{
                        t('shared.token.expires', {
                          time: formatDateTime(token.expires_at),
                        })
                      }}
                    </template>
                  </div>
                  <div class="text-xs text-(--ui-text-dimmed)">
                    {{ token.scopes.join(', ') || t('shared.token.noScopes') }}
                  </div>
                </div>
                <UButton
                  icon="i-lucide-trash-2"
                  variant="ghost"
                  color="error"
                  size="sm"
                  :aria-label="
                    t('shared.token.revokeAriaLabel', {
                      tokenName: token.token_name,
                    })
                  "
                  @click="
                    () => {
                      revokeTarget = token
                    }
                  "
                />
              </div>
            </div>
            <p v-else class="text-sm text-(--ui-text-muted)">
              {{ t('users.tokens.noTokens') }}
            </p>
            <div
              v-if="tokenTotalPages > 1"
              class="mt-3 flex items-center justify-between border-t border-(--ui-border) pt-3"
            >
              <span class="text-xs text-(--ui-text-muted)">
                {{
                  t('shared.token.pagination', {
                    page: tokenPage,
                    totalPages: tokenTotalPages,
                  })
                }}
              </span>
              <UPagination
                v-model:page="tokenPage"
                :total="tokenTotal"
                :items-per-page="PAGE_SIZE"
                @update:page="onTokenPageChange"
              />
            </div>
          </div>
        </div>
      </template>
    </UDrawer>

    <!-- Revoke token confirm -->
    <ConfirmModal
      v-model:open="revokeModalOpen"
      :title="t('users.confirmations.revokeTitle')"
      :description="
        revokeTarget
          ? t('users.confirmations.revokeDescription', {
              tokenName: revokeTarget.token_name,
            })
          : undefined
      "
      :confirm-label="t('shared.token.revoke')"
      confirm-color="error"
      :loading="revoking"
      @confirm="confirmRevokeToken"
    />
  </div>
</template>
