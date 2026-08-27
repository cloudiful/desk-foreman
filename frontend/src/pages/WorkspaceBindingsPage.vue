<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  listAdminWorkspaceBindings,
  transitionAdminWorkspaceBinding,
} from '../api/users'
import { useNotify } from '../composables/useNotify'
import { formatDateTime } from '../utils/format'
import { pageCount, pageOffset } from '../utils/pagination'
import type {
  ListWorkspaceBindingsData,
  WorkspaceBindingResponse,
} from '../generated/openapi/types.gen'

type LifecycleFilter = 'all' | 'active' | 'archived' | 'resetting'
type BindingAction = 'archive' | 'restore' | 'reset'

// Bindings use a smaller page size so the existing pagination is visible
// for normal datasets without changing the shared admin page size.
const BINDING_PAGE_SIZE = 20

const { success, error: notifyError } = useNotify()
const { t } = useI18n()

function truncateId(id: string | null | undefined): string {
  if (!id) return ''
  return id.length > 12 ? `${id.slice(0, 12)}…` : id
}

const rows = ref<WorkspaceBindingResponse[]>([])
const total = ref(0)
const loading = ref(false)
const error = ref('')
const applicationId = ref('')
const externalUserId = ref('')
const workspaceKey = ref('')
const lifecycleFilter = ref<LifecycleFilter>('all')
const page = ref(1)
const actionTarget = ref<{
  binding: WorkspaceBindingResponse
  action: BindingAction
} | null>(null)
const acting = ref(false)
let loadSequence = 0

const totalPages = computed(() => pageCount(total.value, BINDING_PAGE_SIZE))

async function load(): Promise<void> {
  const sequence = ++loadSequence
  loading.value = true
  error.value = ''
  try {
    const query: NonNullable<ListWorkspaceBindingsData['query']> = {
      limit: BINDING_PAGE_SIZE,
      offset: pageOffset(page.value, BINDING_PAGE_SIZE),
      application_id: parseApplicationId(applicationId.value),
      external_user_id: externalUserId.value.trim() || undefined,
      workspace_key: workspaceKey.value.trim() || undefined,
      lifecycle_state:
        lifecycleFilter.value === 'all' ? undefined : lifecycleFilter.value,
    }
    const result = await listAdminWorkspaceBindings(query)
    if (sequence !== loadSequence) return
    rows.value = result.items
    total.value = result.total
  } catch (err) {
    if (sequence === loadSequence) {
      error.value =
        err instanceof Error ? err.message : t('workspaceBindings.errors.load')
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

watch(lifecycleFilter, () => {
  page.value = 1
  void load()
})

function confirmAction(
  binding: WorkspaceBindingResponse,
  action: BindingAction,
): void {
  actionTarget.value = { binding, action }
}

function actionLabel(action: BindingAction): string {
  if (action === 'archive') return t('workspaceBindings.actions.archive')
  if (action === 'restore') return t('workspaceBindings.actions.restore')
  return t('workspaceBindings.actions.reset')
}

function actionError(action: BindingAction): string {
  if (action === 'archive') return t('workspaceBindings.errors.archive')
  if (action === 'restore') return t('workspaceBindings.errors.restore')
  return t('workspaceBindings.errors.reset')
}

function actionSuccess(action: BindingAction): string {
  if (action === 'archive')
    return t('workspaceBindings.notifications.archiveSuccess')
  if (action === 'restore')
    return t('workspaceBindings.notifications.restoreSuccess')
  return t('workspaceBindings.notifications.resetSuccess')
}

function parseApplicationId(value: string): number | undefined {
  if (!value.trim()) return undefined
  const parsed = Number(value)
  return Number.isInteger(parsed) && parsed > 0 ? parsed : undefined
}

async function runAction(): Promise<void> {
  const target = actionTarget.value
  if (!target || acting.value) return
  acting.value = true
  try {
    await transitionAdminWorkspaceBinding(
      target.binding.workspace_binding_id,
      target.action,
    )
    success(
      actionSuccess(target.action),
      t('workspaceBindings.notifications.binding', {
        id: target.binding.workspace_binding_id,
      }),
    )
    actionTarget.value = null
    await load()
  } catch (err) {
    notifyError(
      actionError(target.action),
      err instanceof Error ? err.message : undefined,
    )
  } finally {
    acting.value = false
  }
}

const actionDescription = computed(() => {
  const target = actionTarget.value
  if (!target) return ''
  const id = `#${target.binding.workspace_binding_id}`
  if (target.action === 'archive')
    return t('workspaceBindings.confirmations.archiveDescription', { id })
  if (target.action === 'restore')
    return t('workspaceBindings.confirmations.restoreDescription', { id })
  return t('workspaceBindings.confirmations.resetDescription', { id })
})

const actionTitle = computed(() =>
  actionTarget.value
    ? t('workspaceBindings.confirmations.title', {
        action: actionLabel(actionTarget.value.action),
      })
    : '',
)

const actionConfirmLabel = computed(() =>
  actionTarget.value
    ? actionLabel(actionTarget.value.action)
    : t('shared.confirm.confirm'),
)

const actionModalOpen = computed<boolean>({
  get: () => Boolean(actionTarget.value),
  set: (open) => {
    if (!open) actionTarget.value = null
  },
})

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
          :aria-label="t('workspaceBindings.actions.refresh')"
          @click="load"
        />
      </template>
    </PageHeader>

    <section
      class="rounded-xl border border-(--ui-border) bg-(--ui-bg) shadow-sm"
    >
      <div
        class="flex flex-col flex-wrap gap-3 border-b border-(--ui-border) p-4 md:flex-row md:items-center"
      >
        <UInput
          v-model="applicationId"
          :placeholder="t('workspaceBindings.filters.applicationId')"
          type="number"
          class="w-32"
          @keyup.enter="onSearchEnter"
        />
        <UInput
          v-model="externalUserId"
          :placeholder="t('workspaceBindings.filters.externalUserId')"
          class="md:max-w-xs"
          @keyup.enter="onSearchEnter"
        />
        <UInput
          v-model="workspaceKey"
          :placeholder="t('workspaceBindings.filters.workspaceKey')"
          class="md:max-w-xs"
          @keyup.enter="onSearchEnter"
        />
        <USelect
          v-model="lifecycleFilter"
          :items="[
            {
              label: t('workspaceBindings.filters.allStates'),
              value: 'all',
            },
            { label: t('workspaceBindings.filters.active'), value: 'active' },
            {
              label: t('workspaceBindings.filters.archived'),
              value: 'archived',
            },
            {
              label: t('workspaceBindings.filters.resetting'),
              value: 'resetting',
            },
          ]"
          class="w-36"
        />
        <UButton
          icon="i-lucide-filter"
          variant="soft"
          :loading="loading"
          @click="onSearchEnter"
        >
          {{ t('workspaceBindings.actions.apply') }}
        </UButton>
      </div>

      <ErrorAlert v-if="error" :error="error" class="m-4" @retry="load" />

      <DataTable
        :rows="rows"
        :columns="[
          { key: 'id', label: t('workspaceBindings.table.binding') },
          { key: 'identity', label: t('workspaceBindings.table.identity') },
          { key: 'workspace', label: t('workspaceBindings.table.workspace') },
          { key: 'lifecycle_state', label: t('workspaceBindings.table.state') },
          { key: 'last_used_at', label: t('workspaceBindings.table.lastUsed') },
          { key: 'actions', label: '', class: 'text-right' },
        ]"
        :loading="loading"
        :row-key="(row) => row.workspace_binding_id as number"
        :empty-title="t('workspaceBindings.empty.title')"
        :empty-description="t('workspaceBindings.empty.description')"
      >
        <template #cell-id="{ row }">
          <span class="font-mono text-sm text-(--ui-text-highlighted)">
            #{{ row.workspace_binding_id }}
          </span>
        </template>
        <template #cell-identity="{ row }">
          <div class="min-w-0">
            <div class="text-sm text-(--ui-text-highlighted)">
              {{ t('workspaceBindings.identity.application') }} #{{
                row.application_id
              }}
            </div>
            <div class="truncate text-xs text-(--ui-text-muted)">
              {{ row.external_user_id }}
              <span v-if="row.resource_kind" class="text-(--ui-text-dimmed)">
                · {{ row.resource_kind }}:{{
                  truncateId(row.resource_id as string)
                }}
              </span>
            </div>
          </div>
        </template>
        <template #cell-workspace="{ row }">
          <div class="min-w-0">
            <div class="truncate font-mono text-xs text-(--ui-text-muted)">
              {{ row.workspace_root }}
            </div>
            <div class="text-xs text-(--ui-text-dimmed)">
              {{ row.workspace_key }}
            </div>
          </div>
        </template>
        <template #cell-lifecycle_state="{ row }">
          <StatusBadge :status="row.lifecycle_state as string" />
        </template>
        <template #cell-last_used_at="{ row }">
          <span class="whitespace-nowrap text-sm text-(--ui-text-muted)">
            {{ formatDateTime(row.last_used_at as string) }}
          </span>
        </template>
        <template #cell-actions="{ row }">
          <div class="flex justify-end gap-1">
            <UButton
              v-if="row.lifecycle_state === 'active'"
              icon="i-lucide-archive"
              variant="ghost"
              color="neutral"
              size="sm"
              :aria-label="t('workspaceBindings.actions.archive')"
              @click="
                confirmAction(
                  row as unknown as WorkspaceBindingResponse,
                  'archive',
                )
              "
            />
            <UButton
              v-else
              icon="i-lucide-archive-restore"
              variant="ghost"
              color="neutral"
              size="sm"
              :aria-label="t('workspaceBindings.actions.restore')"
              @click="
                confirmAction(
                  row as unknown as WorkspaceBindingResponse,
                  'restore',
                )
              "
            />
            <UButton
              icon="i-lucide-refresh-ccw"
              variant="ghost"
              color="error"
              size="sm"
              :aria-label="t('workspaceBindings.actions.reset')"
              @click="
                confirmAction(
                  row as unknown as WorkspaceBindingResponse,
                  'reset',
                )
              "
            />
          </div>
        </template>
      </DataTable>

      <div
        v-if="totalPages > 1"
        class="flex items-center justify-between border-t border-(--ui-border) px-4 py-3"
      >
        <span class="text-sm text-(--ui-text-muted)">
          {{ t('workspaceBindings.pagination', { page, totalPages }) }}
        </span>
        <UPagination
          v-model:page="page"
          :total="total"
          :items-per-page="BINDING_PAGE_SIZE"
          @update:page="onPageChange"
        />
      </div>
    </section>

    <ConfirmModal
      v-model:open="actionModalOpen"
      :title="actionTitle"
      :description="actionDescription"
      :confirm-label="actionConfirmLabel"
      :confirm-color="actionTarget?.action === 'reset' ? 'error' : 'primary'"
      :loading="acting"
      @confirm="runAction"
    />
  </div>
</template>
