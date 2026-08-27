<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

export interface DataColumn {
  key: string
  label: string
  class?: string
  sortable?: boolean
}

const props = withDefaults(
  defineProps<{
    columns: DataColumn[]
    rows: unknown[]
    loading?: boolean
    emptyTitle?: string
    emptyDescription?: string
    rowKey?: string | ((row: Record<string, unknown>) => string | number)
  }>(),
  {
    loading: false,
    emptyTitle: undefined,
    emptyDescription: undefined,
    rowKey: 'id',
  },
)

const { t } = useI18n()
const emptyTitle = computed(
  () => props.emptyTitle ?? t('shared.dataTable.noData'),
)
const emptyDescription = computed(
  () => props.emptyDescription ?? t('shared.dataTable.nothingToShow'),
)

const sort = defineModel<{ key: string; dir: 'asc' | 'desc' } | null>('sort', {
  default: null,
})

const typedRows = computed(() => props.rows as Record<string, unknown>[])

function keyFor(row: Record<string, unknown>): string | number {
  return typeof props.rowKey === 'function'
    ? props.rowKey(row)
    : (row[props.rowKey] as string | number)
}

function toggleSort(column: DataColumn): void {
  if (!column.sortable) return
  if (!sort.value || sort.value.key !== column.key) {
    sort.value = { key: column.key, dir: 'asc' }
  } else if (sort.value.dir === 'asc') {
    sort.value = { key: column.key, dir: 'desc' }
  } else {
    sort.value = null
  }
}
</script>

<template>
  <div class="overflow-x-auto">
    <table class="w-full min-w-[560px] border-collapse text-sm">
      <thead>
        <tr class="border-b border-(--ui-border) bg-(--ui-bg-elevated)/60">
          <th
            v-for="column in columns"
            :key="column.key"
            :class="[
              'whitespace-nowrap px-4 py-2.5 text-left font-medium text-(--ui-text-muted)',
              column.class,
            ]"
          >
            <button
              v-if="column.sortable"
              type="button"
              class="group inline-flex items-center gap-1.5 rounded hover:text-(--ui-text-highlighted)"
              @click="toggleSort(column)"
            >
              {{ column.label }}
              <UIcon
                :name="
                  sort?.key === column.key
                    ? sort?.dir === 'asc'
                      ? 'i-lucide-arrow-up'
                      : 'i-lucide-arrow-down'
                    : 'i-lucide-chevrons-up-down'
                "
                :class="[
                  'size-3.5 text-(--ui-text-dimmed)',
                  sort?.key === column.key
                    ? 'opacity-100'
                    : 'opacity-0 group-hover:opacity-60',
                ]"
              />
            </button>
            <span v-else>{{ column.label }}</span>
          </th>
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="row in typedRows"
          :key="keyFor(row)"
          class="border-b border-(--ui-border-muted) transition-colors hover:bg-(--ui-bg-elevated)/50"
        >
          <td
            v-for="column in columns"
            :key="column.key"
            :class="['px-4 py-3 align-middle text-(--ui-text)', column.class]"
          >
            <slot
              :name="`cell-${column.key}`"
              :row="row"
              :value="row[column.key]"
            >
              {{ row[column.key] ?? '—' }}
            </slot>
          </td>
        </tr>
        <tr v-if="!loading && !typedRows.length">
          <td :colspan="columns.length" class="p-0">
            <EmptyState :title="emptyTitle" :description="emptyDescription" />
          </td>
        </tr>
      </tbody>
    </table>
    <div
      v-if="loading"
      class="flex items-center justify-center gap-2 py-8 text-sm text-(--ui-text-muted)"
    >
      <UIcon name="i-lucide-loader-2" class="size-4 animate-spin" />
      {{ t('shared.dataTable.loading') }}
    </div>
  </div>
</template>
