<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { listAdminAuditLogs } from '../api/users'
import { formatDateTime, formatMilliseconds } from '../utils/format'
import { AUDIT_PAGE_SIZE, pageCount, pageOffset } from '../utils/pagination'
import type { AuditLogResponse } from '../generated/openapi/types.gen'

const { t } = useI18n()

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
        err instanceof Error && err.message
          ? err.message
          : t('audit.errors.load')
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

function statusLabel(status: string | null | undefined): string {
  if (status === 'success') return t('audit.status.success')
  if (status === 'failure') return t('audit.status.failure')
  if (status === 'unknown') return t('audit.status.unknown')
  return status ?? t('audit.notAvailable')
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
    <PageHeader>
      <template #actions>
        <UButton
          icon="i-lucide-refresh-cw"
          variant="outline"
          color="neutral"
          :loading="loading"
          :aria-label="t('audit.refresh')"
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
          :placeholder="t('audit.filterPlaceholder')"
          leading-icon="i-lucide-search"
          class="md:max-w-sm"
        />
        <USelect
          v-model="statusFilter"
          :items="[
            { label: t('audit.outcomes.all'), value: 'all' },
            { label: t('audit.outcomes.success'), value: 'success' },
            { label: t('audit.outcomes.failure'), value: 'failure' },
            { label: t('audit.outcomes.unknown'), value: 'unknown' },
          ]"
          class="w-36"
        />
        <span class="ml-auto text-sm text-(--ui-text-muted)">
          {{ t('audit.eventsCount', { shown: rows.length, total }) }}
        </span>
      </div>

      <ErrorAlert v-if="error" :error="error" class="m-4" @retry="load" />

      <DataTable
        :rows="rows"
        :columns="[
          { key: 'created_at', label: t('audit.columns.time') },
          { key: 'action', label: t('audit.columns.action') },
          { key: 'actor', label: t('audit.columns.actor') },
          { key: 'target', label: t('audit.columns.target') },
          { key: 'status', label: t('audit.columns.outcome') },
          { key: 'actions', label: '', class: 'text-right' },
        ]"
        :loading="loading"
        :row-key="(row) => row.audit_id as number"
        :empty-title="t('audit.noEvents')"
        :empty-description="t('audit.noEventsDescription')"
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
            {{ statusLabel(row.status as string | null) }}
          </span>
        </template>
        <template #cell-actions="{ row }">
          <div class="flex justify-end">
            <UButton
              icon="i-lucide-eye"
              variant="ghost"
              color="neutral"
              size="sm"
              :aria-label="t('audit.detailsAriaLabel')"
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
          {{ t('audit.page', { page, totalPages }) }}
        </span>
        <UPagination
          v-model:page="page"
          :total="total"
          :items-per-page="AUDIT_PAGE_SIZE"
          @update:page="onPageChange"
        />
      </div>
    </section>

    <UDrawer
      v-model:open="detailOpen"
      :title="t('audit.detailTitle')"
      :close="true"
    >
      <template #body>
        <dl v-if="detail" class="space-y-3 text-sm">
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">{{ t('audit.detail.time') }}</dt>
            <dd>{{ formatDateTime(detail.created_at) }}</dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('audit.detail.action') }}
            </dt>
            <dd>
              <code class="font-mono">{{ detail.action }}</code>
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('audit.detail.actor') }}
            </dt>
            <dd>
              {{ detail.actor_type }}
              <span v-if="detail.actor_user_id" class="font-mono text-xs"
                >#{{ detail.actor_user_id }}</span
              >
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('audit.detail.target') }}
            </dt>
            <dd class="max-w-[55%] break-all font-mono text-right">
              {{ detail.target_type }} {{ detail.target_id }}
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('audit.detail.outcome') }}
            </dt>
            <dd :class="statusColor(detail.status)">
              {{ statusLabel(detail.status) }}
            </dd>
          </div>
          <div class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('audit.detail.duration') }}
            </dt>
            <dd>{{ formatMilliseconds(detail.duration_ms) }}</dd>
          </div>
          <div v-if="detail.request_id" class="flex justify-between gap-4">
            <dt class="text-(--ui-text-muted)">
              {{ t('audit.detail.requestId') }}
            </dt>
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
              {{ t('audit.detail.payload') }}
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
