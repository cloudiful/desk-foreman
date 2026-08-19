<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { listAdminAuditLogs } from '../api/users'
import { formatDateTime, formatMilliseconds } from '../utils/format'
import { AUDIT_PAGE_SIZE, pageCount, pageOffset } from '../utils/pagination'
import type { AuditLogResponse } from '../generated/openapi/types.gen'

function truncateId(id: string): string {
  if (!id) return ''
  return id.length > 14 ? `${id.slice(0, 14)}…` : id
}

const rows = ref<AuditLogResponse[]>([])
const loading = ref(false)
const error = ref('')
const total = ref(0)
const page = ref(1)
const actionFilter = ref('')
const statusFilter = ref<'all' | 'success' | 'failure' | 'unknown'>('all')
const detail = ref<AuditLogResponse | null>(null)
let loadSequence = 0
let filterTimer: ReturnType<typeof setTimeout> | undefined

const totalPages = computed(() => pageCount(total.value, AUDIT_PAGE_SIZE))

async function load(): Promise<void> {
  const sequence = ++loadSequence
  loading.value = true
  error.value = ''
  try {
    const result = await listAdminAuditLogs({
      limit: AUDIT_PAGE_SIZE,
      offset: pageOffset(page.value, AUDIT_PAGE_SIZE),
      search: actionFilter.value.trim() || undefined,
      status: statusFilter.value === 'all' ? undefined : statusFilter.value,
    })
    if (sequence !== loadSequence) return
    rows.value = result.items
    total.value = result.total
  } catch (err) {
    if (sequence === loadSequence) {
      error.value =
        err instanceof Error ? err.message : 'Failed to load audit logs'
    }
  } finally {
    if (sequence === loadSequence) loading.value = false
  }
}

function onPageChange(): void {
  void load()
}

watch([actionFilter, statusFilter], () => {
  page.value = 1
  if (filterTimer) clearTimeout(filterTimer)
  filterTimer = setTimeout(() => void load(), 250)
})

function statusColor(status: string | null | undefined): string {
  if (status === 'success') return 'text-(--ui-text-success)'
  if (status && status !== 'unknown') return 'text-(--ui-text-error)'
  return 'text-(--ui-text-muted)'
}

function payloadPreview(payload: unknown): string {
  if (payload === null || payload === undefined) return ''
  try {
    return JSON.stringify(payload)
  } catch {
    return String(payload)
  }
}

const detailOpen = computed<boolean>({
  get: () => Boolean(detail.value),
  set: (open) => {
    if (!open) detail.value = null
  },
})

onMounted(() => void load())

onUnmounted(() => {
  if (filterTimer) clearTimeout(filterTimer)
})
</script>

<template>
  <div class="space-y-6">
    <PageHeader
      title="Audit log"
      description="Immutable trail of control-plane activity"
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
        class="flex flex-col gap-3 border-b border-(--ui-border) p-4 md:flex-row md:items-center"
      >
        <UInput
          v-model="actionFilter"
          placeholder="Filter by action, actor or target…"
          leading-icon="i-lucide-search"
          class="md:max-w-sm"
        />
        <USelect
          v-model="statusFilter"
          :items="[
            { label: 'All outcomes', value: 'all' },
            { label: 'Success', value: 'success' },
            { label: 'Failure', value: 'failure' },
            { label: 'Unknown', value: 'unknown' },
          ]"
          class="w-36"
        />
        <span class="ml-auto text-sm text-(--ui-text-muted)">
          {{ rows.length }} of {{ total }} events
        </span>
      </div>

      <ErrorAlert v-if="error" :error="error" class="m-4" @retry="load" />

      <DataTable
        :rows="rows"
        :columns="[
          { key: 'created_at', label: 'Time' },
          { key: 'action', label: 'Action' },
          { key: 'actor', label: 'Actor' },
          { key: 'target', label: 'Target' },
          { key: 'status', label: 'Outcome' },
          { key: 'actions', label: '', class: 'text-right' },
        ]"
        :loading="loading"
        :row-key="(row) => row.audit_id as number"
        empty-title="No audit events"
        empty-description="Events appear as actions are performed on the control plane."
      >
        <template #cell-created_at="{ row }">
          <span class="whitespace-nowrap text-sm text-(--ui-text-muted)">
            {{ formatDateTime(row.created_at as string) }}
          </span>
        </template>
        <template #cell-action="{ row }">
          <UBadge variant="soft" color="neutral" size="sm">
            <code class="font-mono">{{ row.action }}</code>
          </UBadge>
        </template>
        <template #cell-actor="{ row }">
          <span class="text-sm text-(--ui-text-muted)">
            {{ row.actor_type }}
            <span v-if="row.actor_user_id" class="font-mono text-xs"
              >#{{ row.actor_user_id }}</span
            >
            <span v-else-if="row.actor_application_id" class="font-mono text-xs"
              >#{{ row.actor_application_id }}</span
            >
          </span>
        </template>
        <template #cell-target="{ row }">
          <span class="text-sm text-(--ui-text-muted)">
            {{ row.target_type }}
            <span class="font-mono text-xs"
              >#{{ truncateId(row.target_id as string) }}</span
            >
          </span>
        </template>
        <template #cell-status="{ row }">
          <span
            class="text-sm"
            :class="statusColor(row.status as string | null)"
          >
            {{ row.status ?? '—' }}
          </span>
        </template>
        <template #cell-actions="{ row }">
          <div class="flex justify-end">
            <UButton
              icon="i-lucide-eye"
              variant="ghost"
              color="neutral"
              size="sm"
              aria-label="Audit event details"
              @click="
                () => {
                  detail = row as unknown as AuditLogResponse
                }
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
          :items-per-page="AUDIT_PAGE_SIZE"
          @update:page="onPageChange"
        />
      </div>
    </section>

    <UDrawer v-model:open="detailOpen" title="Audit event" :close="true">
      <template #body>
        <dl v-if="detail" class="space-y-3 text-sm">
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Time</dt>
            <dd>{{ formatDateTime(detail.created_at) }}</dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Action</dt>
            <dd>
              <code class="font-mono">{{ detail.action }}</code>
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Actor</dt>
            <dd>
              {{ detail.actor_type }}
              <span v-if="detail.actor_user_id" class="font-mono text-xs"
                >#{{ detail.actor_user_id }}</span
              >
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Target</dt>
            <dd class="max-w-[55%] break-all font-mono text-right">
              {{ detail.target_type }} {{ detail.target_id }}
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Outcome</dt>
            <dd :class="statusColor(detail.status)">
              {{ detail.status ?? '—' }}
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Duration</dt>
            <dd>{{ formatMilliseconds(detail.duration_ms) }}</dd>
          </div>
          <div v-if="detail.request_id" class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">Request ID</dt>
            <dd class="max-w-[55%] break-all font-mono text-right text-xs">
              {{ detail.request_id }}
            </dd>
          </div>
          <div
            class="rounded-lg border border-(--ui-border) bg-(--ui-bg-elevated) p-3"
          >
            <dt
              class="mb-2 text-xs font-semibold uppercase tracking-wide text-(--ui-text-muted)"
            >
              Payload
            </dt>
            <dd>
              <pre
                class="max-h-72 overflow-auto whitespace-pre-wrap font-mono text-xs leading-relaxed text-(--ui-text)"
                >{{ payloadPreview(detail.payload) }}</pre>
            </dd>
          </div>
        </dl>
      </template>
    </UDrawer>
  </div>
</template>
