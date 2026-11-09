<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  createAdminWorkspaceGitUpstream,
  deleteAdminWorkspaceGitUpstream,
  listAdminWorkspaceGitUpstreams,
  setAdminWorkspaceGitUpstreamEnabled,
  updateAdminWorkspaceGitUpstream,
} from '../api/users'
import { useNotify } from '../composables/useNotify'
import type {
  WorkspaceBindingResponse,
  WorkspaceGitUpstreamResponse,
} from '../generated/openapi/types.gen'

const props = defineProps<{
  binding: WorkspaceBindingResponse | null
}>()

const open = defineModel<boolean>('open', { default: false })
const { t } = useI18n()
const { success, error: notifyError } = useNotify()

const rows = ref<WorkspaceGitUpstreamResponse[]>([])
const loading = ref(false)
const saving = ref(false)
const listError = ref('')
const formError = ref('')

const editingId = ref<number | null>(null)
const alias = ref('')
const targetUrl = ref('')
const credentialUsername = ref('')
const credentialSecret = ref('')
const clearCredential = ref(false)

const deleteTarget = ref<WorkspaceGitUpstreamResponse | null>(null)
const deleting = ref(false)
const togglingId = ref<number | null>(null)

const readOnly = computed(
  () =>
    !props.binding ||
    props.binding.lifecycle_state !== 'active' ||
    !props.binding.is_active,
)

const title = computed(() =>
  t('workspaceBindings.upstreams.title', {
    id: props.binding?.workspace_binding_id ?? '',
  }),
)

const deleteDescription = computed(() =>
  deleteTarget.value
    ? t('workspaceBindings.upstreams.confirmDelete.description', {
        alias: deleteTarget.value.remote_alias,
      })
    : '',
)

watch(
  () => [open.value, props.binding?.workspace_binding_id] as const,
  ([isOpen]) => {
    if (!isOpen) return
    resetForm()
    void load()
  },
)

function resetForm(): void {
  editingId.value = null
  alias.value = ''
  targetUrl.value = ''
  credentialUsername.value = ''
  credentialSecret.value = ''
  clearCredential.value = false
  formError.value = ''
}

async function load(): Promise<void> {
  const binding = props.binding
  if (!binding) {
    rows.value = []
    return
  }
  loading.value = true
  listError.value = ''
  try {
    const page = await listAdminWorkspaceGitUpstreams(
      binding.workspace_binding_id,
      { limit: 200, offset: 0 },
    )
    rows.value = page.items
  } catch (err) {
    listError.value =
      err instanceof Error
        ? err.message
        : t('workspaceBindings.errors.loadUpstreams')
  } finally {
    loading.value = false
  }
}

function startCreate(): void {
  resetForm()
}

function startEdit(row: WorkspaceGitUpstreamResponse): void {
  editingId.value = row.upstream_id
  alias.value = row.remote_alias
  targetUrl.value = row.target_url
  credentialUsername.value = ''
  credentialSecret.value = ''
  clearCredential.value = false
  formError.value = ''
}

function validateCredentialPair(requireSecret: boolean): string {
  const hasUsername = credentialUsername.value.length > 0
  const hasSecret = credentialSecret.value.length > 0
  if (clearCredential.value) {
    if (hasUsername || hasSecret) {
      return t('workspaceBindings.upstreams.errors.clearConflict')
    }
    return ''
  }
  if (hasUsername !== hasSecret) {
    return t('workspaceBindings.upstreams.errors.pairRequired')
  }
  if (requireSecret && hasSecret && !hasUsername) {
    return t('workspaceBindings.upstreams.errors.pairRequired')
  }
  return ''
}

async function save(): Promise<void> {
  const binding = props.binding
  if (!binding || readOnly.value || saving.value) return
  formError.value = validateCredentialPair(editingId.value === null)
  if (formError.value) return

  const hasCredential =
    credentialUsername.value.length > 0 && credentialSecret.value.length > 0
  saving.value = true
  try {
    if (editingId.value === null) {
      await createAdminWorkspaceGitUpstream(binding.workspace_binding_id, {
        remote_alias: alias.value.trim(),
        target_url: targetUrl.value.trim(),
        credential_username: hasCredential
          ? credentialUsername.value
          : undefined,
        credential_secret: hasCredential ? credentialSecret.value : undefined,
      })
      success(t('workspaceBindings.upstreams.notifications.created'))
    } else {
      await updateAdminWorkspaceGitUpstream(
        binding.workspace_binding_id,
        editingId.value,
        {
          remote_alias: alias.value.trim(),
          target_url: targetUrl.value.trim(),
          credential_username: hasCredential
            ? credentialUsername.value
            : undefined,
          credential_secret: hasCredential ? credentialSecret.value : undefined,
          clear_credential: clearCredential.value || undefined,
        },
      )
      success(t('workspaceBindings.upstreams.notifications.updated'))
    }
    resetForm()
    await load()
  } catch (err) {
    formError.value =
      err instanceof Error
        ? err.message
        : t('workspaceBindings.errors.saveUpstream')
  } finally {
    saving.value = false
  }
}

async function toggle(row: WorkspaceGitUpstreamResponse): Promise<void> {
  const binding = props.binding
  if (!binding || readOnly.value || togglingId.value !== null) return
  togglingId.value = row.upstream_id
  try {
    await setAdminWorkspaceGitUpstreamEnabled(
      binding.workspace_binding_id,
      row.upstream_id,
      !row.enabled,
    )
    success(
      t(
        row.enabled
          ? 'workspaceBindings.upstreams.notifications.disabled'
          : 'workspaceBindings.upstreams.notifications.enabled',
      ),
    )
    await load()
  } catch (err) {
    notifyError(
      t('workspaceBindings.errors.toggleUpstream'),
      err instanceof Error ? err.message : undefined,
    )
  } finally {
    togglingId.value = null
  }
}

async function confirmDelete(): Promise<void> {
  const binding = props.binding
  const target = deleteTarget.value
  if (!binding || !target || deleting.value) return
  deleting.value = true
  try {
    await deleteAdminWorkspaceGitUpstream(
      binding.workspace_binding_id,
      target.upstream_id,
    )
    success(t('workspaceBindings.upstreams.notifications.deleted'))
    if (editingId.value === target.upstream_id) resetForm()
    deleteTarget.value = null
    await load()
  } catch (err) {
    notifyError(
      t('workspaceBindings.errors.deleteUpstream'),
      err instanceof Error ? err.message : undefined,
    )
  } finally {
    deleting.value = false
  }
}

const deleteModalOpen = computed<boolean>({
  get: () => Boolean(deleteTarget.value),
  set: (value) => {
    if (!value) deleteTarget.value = null
  },
})
</script>

<template>
  <UDrawer v-model:open="open" :title="title" :dismissible="!saving">
    <template #body>
      <div class="space-y-6">
        <UAlert
          v-if="readOnly"
          color="warning"
          variant="subtle"
          :title="t('workspaceBindings.upstreams.archived.title')"
          :description="t('workspaceBindings.upstreams.archived.description')"
        />

        <ErrorAlert
          v-if="listError"
          :error="listError"
          @retry="load"
        />

        <div class="flex items-center justify-between gap-3">
          <p class="text-sm text-(--ui-text-muted)">
            {{ t('workspaceBindings.upstreams.hint') }}
          </p>
          <UButton
            icon="i-lucide-plus"
            size="sm"
            :disabled="readOnly"
            :aria-label="t('workspaceBindings.upstreams.actions.add')"
            @click="startCreate"
          >
            {{ t('workspaceBindings.upstreams.actions.add') }}
          </UButton>
        </div>

        <DataTable
          :rows="rows"
          :columns="[
            {
              key: 'remote_alias',
              label: t('workspaceBindings.upstreams.table.alias'),
            },
            {
              key: 'target_url',
              label: t('workspaceBindings.upstreams.table.targetUrl'),
            },
            {
              key: 'enabled',
              label: t('workspaceBindings.upstreams.table.enabled'),
            },
            {
              key: 'credential',
              label: t('workspaceBindings.upstreams.table.credential'),
            },
            {
              key: 'actions',
              label: '',
              class: 'text-right',
            },
          ]"
          :loading="loading"
          :row-key="(row) => row.upstream_id as number"
          :empty-title="t('workspaceBindings.upstreams.empty.title')"
          :empty-description="t('workspaceBindings.upstreams.empty.description')"
        >
          <template #cell-remote_alias="{ row }">
            <span class="font-mono text-sm text-(--ui-text-highlighted)">
              {{ row.remote_alias }}
            </span>
          </template>
          <template #cell-target_url="{ row }">
            <span class="block max-w-md truncate font-mono text-xs">
              {{ row.target_url }}
            </span>
          </template>
          <template #cell-enabled="{ row }">
            <UBadge color="neutral" variant="subtle" size="sm">
              {{
                row.enabled
                  ? t('workspaceBindings.upstreams.state.enabled')
                  : t('workspaceBindings.upstreams.state.disabled')
              }}
            </UBadge>
          </template>
          <template #cell-credential="{ row }">
            <span class="text-sm text-(--ui-text-muted)">
              {{
                row.has_credential
                  ? t('workspaceBindings.upstreams.credential.present')
                  : t('workspaceBindings.upstreams.credential.absent')
              }}
            </span>
          </template>
          <template #cell-actions="{ row }">
            <div class="flex justify-end gap-1">
              <UButton
                icon="i-lucide-pencil"
                variant="ghost"
                color="neutral"
                size="sm"
                :disabled="readOnly"
                :aria-label="t('workspaceBindings.upstreams.actions.edit')"
                @click="
                  startEdit(row as unknown as WorkspaceGitUpstreamResponse)
                "
              />
              <UButton
                :icon="
                  row.enabled ? 'i-lucide-toggle-right' : 'i-lucide-toggle-left'
                "
                variant="ghost"
                color="neutral"
                size="sm"
                :loading="togglingId === row.upstream_id"
                :disabled="readOnly"
                :aria-label="
                  row.enabled
                    ? t('workspaceBindings.upstreams.actions.disable')
                    : t('workspaceBindings.upstreams.actions.enable')
                "
                @click="
                  toggle(row as unknown as WorkspaceGitUpstreamResponse)
                "
              />
              <UButton
                icon="i-lucide-trash-2"
                variant="ghost"
                color="error"
                size="sm"
                :disabled="readOnly"
                :aria-label="t('workspaceBindings.upstreams.actions.delete')"
                @click="
                  deleteTarget =
                    row as unknown as WorkspaceGitUpstreamResponse
                "
              />
            </div>
          </template>
        </DataTable>

        <form
          v-if="!readOnly"
          id="workspace-git-upstream-form"
          class="space-y-4 rounded-xl border border-(--ui-border) p-4"
          @submit.prevent="save"
        >
          <h3 class="text-sm font-semibold text-(--ui-text-highlighted)">
            {{
              editingId === null
                ? t('workspaceBindings.upstreams.form.createTitle')
                : t('workspaceBindings.upstreams.form.editTitle')
            }}
          </h3>

          <UFormField
            :label="t('workspaceBindings.upstreams.fields.alias')"
            required
          >
            <UInput
              v-model="alias"
              class="w-full"
              autocomplete="off"
              :placeholder="t('workspaceBindings.upstreams.fields.aliasHint')"
            />
          </UFormField>

          <UFormField
            :label="t('workspaceBindings.upstreams.fields.targetUrl')"
            required
          >
            <UInput
              v-model="targetUrl"
              class="w-full font-mono"
              autocomplete="off"
              :placeholder="
                t('workspaceBindings.upstreams.fields.targetUrlHint')
              "
            />
          </UFormField>

          <UFormField
            :label="t('workspaceBindings.upstreams.fields.username')"
            :hint="t('workspaceBindings.upstreams.fields.credentialHint')"
          >
            <UInput
              v-model="credentialUsername"
              class="w-full"
              autocomplete="off"
              :disabled="clearCredential"
            />
          </UFormField>

          <UFormField :label="t('workspaceBindings.upstreams.fields.secret')">
            <UInput
              v-model="credentialSecret"
              type="password"
              class="w-full"
              autocomplete="new-password"
              :disabled="clearCredential"
              :placeholder="
                t('workspaceBindings.upstreams.fields.secretHint')
              "
            />
          </UFormField>

          <UCheckbox
            v-if="editingId !== null"
            v-model="clearCredential"
            :label="t('workspaceBindings.upstreams.fields.clearCredential')"
          />

          <UAlert
            v-if="formError"
            color="error"
            variant="subtle"
            :title="formError"
          />

          <div class="flex justify-end gap-2">
            <UButton
              type="button"
              variant="outline"
              color="neutral"
              :disabled="saving"
              @click="resetForm"
            >
              {{ t('workspaceBindings.upstreams.actions.cancel') }}
            </UButton>
            <UButton type="submit" :loading="saving" :disabled="saving">
              {{
                editingId === null
                  ? t('workspaceBindings.upstreams.actions.create')
                  : t('workspaceBindings.upstreams.actions.save')
              }}
            </UButton>
          </div>
        </form>
      </div>
    </template>
  </UDrawer>

  <ConfirmModal
    v-model:open="deleteModalOpen"
    :title="t('workspaceBindings.upstreams.confirmDelete.title')"
    :description="deleteDescription"
    :confirm-label="t('workspaceBindings.upstreams.actions.delete')"
    confirm-color="error"
    :loading="deleting"
    @confirm="confirmDelete"
  />
</template>
