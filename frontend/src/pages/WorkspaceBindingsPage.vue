<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import {
  listAdminWorkspaceBindings,
  transitionAdminWorkspaceBinding,
} from '../api/users'
import { useNotify } from '../composables/useNotify'
import { formatDateTime } from '../utils/format'
import { PAGE_SIZE, pageCount, pageOffset } from '../utils/pagination'
import type {
  ListWorkspaceBindingsData,
  WorkspaceBindingResponse,
} from '../generated/openapi/types.gen'

type LifecycleFilter = 'all' | 'active' | 'archived' | 'resetting'

const { success, error: notifyError } = useNotify()

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
  action: 'archive' | 'restore' | 'reset'
} | null>(null)
const acting = ref(false)
let loadSequence = 0

const totalPages = computed(() => pageCount(total.value, PAGE_SIZE))

async function load(): Promise<void> {
  const sequence = ++loadSequence
  loading.value = true
  error.value = ''
  try {
    const query: NonNullable<ListWorkspaceBindingsData['query']> = {
      limit: PAGE_SIZE,
      offset: pageOffset(page.value, PAGE_SIZE),
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
        err instanceof Error ? err.message : 'Failed to load workspace bindings'
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
  action: 'archive' | 'restore' | 'reset',
): void {
  actionTarget.value = { binding, action }
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
      `${target.action[0].toUpperCase()}${target.action.slice(1)}d workspace`,
      `Binding #${target.binding.workspace_binding_id}`,
    )
    actionTarget.value = null
    await load()
  } catch (err) {
    notifyError(
      `Failed to ${target.action} workspace`,
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
    return `${id} is archived: the workspace is detached and stops accepting activity.`
  if (target.action === 'restore')
    return `${id} is restored and becomes active again.`
  return `${id} is reset: the workspace directory is cleared. This cannot be undone.`
})

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
    <PageHeader
      title="Workspace bindings"
      description="Workspaces tied to applications and external users"
    >
      <template #actions>
        <UButton
          icon="i-lucide-refresh-cw"
          variant="outline"
          color="neutral"
          :loading="loading"
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
          placeholder="Application ID"
          type="number"
          class="w-32"
          @keyup.enter="onSearchEnter"
        />
        <UInput
          v-model="externalUserId"
          placeholder="External user ID"
          class="md:max-w-xs"
          @keyup.enter="onSearchEnter"
        />
        <UInput
          v-model="workspaceKey"
          placeholder="Workspace key"
          class="md:max-w-xs"
          @keyup.enter="onSearchEnter"
        />
        <USelect
          v-model="lifecycleFilter"
          :items="[
            { label: 'All states', value: 'all' },
            { label: 'Active', value: 'active' },
            { label: 'Archived', value: 'archived' },
            { label: 'Resetting', value: 'resetting' },
          ]"
          class="w-36"
        />
        <UButton
          icon="i-lucide-filter"
          variant="soft"
          :loading="loading"
          @click="onSearchEnter"
        >
          Apply
        </UButton>
      </div>

      <ErrorAlert v-if="error" :error="error" class="m-4" @retry="load" />

      <DataTable
        :rows="rows"
        :columns="[
          { key: 'id', label: 'Binding' },
          { key: 'identity', label: 'Identity' },
          { key: 'workspace', label: 'Workspace' },
          { key: 'lifecycle_state', label: 'State' },
          { key: 'last_used_at', label: 'Last used' },
          { key: 'actions', label: '', class: 'text-right' },
        ]"
        :loading="loading"
        :row-key="(row) => row.workspace_binding_id as number"
        empty-title="No workspace bindings"
        empty-description="Bindings appear when applications access workspaces."
      >
        <template #cell-id="{ row }">
          <span class="font-mono text-sm text-(--ui-text-highlighted)">
            #{{ row.workspace_binding_id }}
          </span>
        </template>
        <template #cell-identity="{ row }">
          <div class="min-w-0">
            <div class="text-sm text-(--ui-text-highlighted)">
              App #{{ row.application_id }}
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
              aria-label="Archive workspace"
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
              aria-label="Restore workspace"
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
              aria-label="Reset workspace"
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
          Page {{ page }} of {{ totalPages }}
        </span>
        <UPagination
          v-model:page="page"
          :total="total"
          :items-per-page="PAGE_SIZE"
          @update:page="onPageChange"
        />
      </div>
    </section>

    <ConfirmModal
      v-model:open="actionModalOpen"
      :title="
        actionTarget
          ? `${actionTarget.action[0].toUpperCase()}${actionTarget.action.slice(1)} workspace`
          : ''
      "
      :description="actionDescription"
      :confirm-label="
        actionTarget
          ? `${actionTarget.action[0].toUpperCase()}${actionTarget.action.slice(1)}`
          : 'Confirm'
      "
      :confirm-color="actionTarget?.action === 'reset' ? 'error' : 'primary'"
      :loading="acting"
      @confirm="runAction"
    />
  </div>
</template>
