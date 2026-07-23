<script setup lang="ts">
import Column from 'primevue/column'
import DataTable from 'primevue/datatable'
import { onMounted, ref } from 'vue'
import { listAdminAuditLogs } from '../api/users'
import type { AuditLogResponse } from '../generated/openapi/types.gen'

const rows = ref<AuditLogResponse[]>([])
const loading = ref(false)
const error = ref('')

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    const page = await listAdminAuditLogs({ limit: 100, offset: 0 })
    rows.value = page.items
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to load audit logs'
  } finally {
    loading.value = false
  }
}

onMounted(() => void load())
</script>

<template>
  <section class="space-y-4">
    <div class="app-shell-panel rounded-[2rem] p-5">
      <div class="text-xs uppercase tracking-[0.25em] text-[var(--muted)]">
        Admin
      </div>
      <h2 class="mt-2 text-2xl font-semibold">Audit</h2>
      <p v-if="error" class="mt-3 text-sm text-red-700">{{ error }}</p>
    </div>
    <div class="app-shell-panel overflow-hidden rounded-[2rem] p-3">
      <DataTable :value="rows" :loading="loading" striped-rows>
        <Column field="created_at" header="Time" />
        <Column field="action" header="Action" />
        <Column field="actor_type" header="Actor" />
        <Column field="target_type" header="Target" />
        <Column field="target_id" header="Target ID" />
        <Column field="status" header="Status" />
        <Column field="duration_ms" header="Duration (ms)" />
      </DataTable>
    </div>
  </section>
</template>
