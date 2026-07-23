<script setup lang="ts">
import Button from 'primevue/button'
import Column from 'primevue/column'
import DataTable from 'primevue/datatable'
import InputText from 'primevue/inputtext'
import { onMounted, ref } from 'vue'
import {
  listAdminWorkspaceBindings,
  transitionAdminWorkspaceBinding,
} from '../api/users'
import type { WorkspaceBindingResponse } from '../generated/openapi/types.gen'

const rows = ref<WorkspaceBindingResponse[]>([])
const loading = ref(false)
const error = ref('')
const applicationId = ref('')
const externalUserId = ref('')
const workspaceKey = ref('')
const busyId = ref<number | null>(null)

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    rows.value = await listAdminWorkspaceBindings({
      limit: 50,
      offset: 0,
      application_id: applicationId.value
        ? Number(applicationId.value)
        : undefined,
      external_user_id: externalUserId.value.trim() || undefined,
      workspace_key: workspaceKey.value.trim() || undefined,
    })
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to load workspace bindings'
  } finally {
    loading.value = false
  }
}

async function transition(
  bindingId: number,
  action: 'archive' | 'restore' | 'reset',
): Promise<void> {
  busyId.value = bindingId
  error.value = ''
  try {
    await transitionAdminWorkspaceBinding(bindingId, action)
    await load()
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : `Failed to ${action} workspace`
  } finally {
    busyId.value = null
  }
}

onMounted(() => {
  void load()
})
</script>

<template>
  <section class="space-y-4">
    <div class="app-shell-panel rounded-[2rem] p-5">
      <div>
        <div class="text-xs uppercase tracking-[0.25em] text-[var(--muted)]">
          Admin
        </div>
        <h2 class="mt-2 text-2xl font-semibold">Workspace bindings</h2>
      </div>
      <div class="mt-4 grid gap-3 md:grid-cols-4">
        <InputText v-model="applicationId" placeholder="Application ID" />
        <InputText v-model="externalUserId" placeholder="External user ID" />
        <InputText v-model="workspaceKey" placeholder="Workspace key" />
        <button
          class="rounded-2xl bg-[var(--accent)] px-4 py-2 text-sm text-white"
          @click="load"
        >
          Apply filters
        </button>
      </div>
      <p v-if="error" class="mt-3 text-sm text-red-700">{{ error }}</p>
    </div>

    <div class="app-shell-panel overflow-hidden rounded-[2rem] p-3">
      <DataTable :value="rows" :loading="loading" striped-rows>
        <Column field="workspace_binding_id" header="Binding ID" />
        <Column field="application_id" header="Application ID" />
        <Column field="external_user_id" header="External user ID" />
        <Column field="workspace_key" header="Workspace key" />
        <Column field="workspace_root" header="Workspace root" />
        <Column field="lifecycle_state" header="State" />
        <Column field="last_used_at" header="Last used" />
        <Column header="Actions">
          <template #body="{ data }">
            <div class="flex gap-2">
              <Button
                v-if="data.lifecycle_state === 'active'"
                label="Archive"
                size="small"
                severity="secondary"
                :loading="busyId === data.workspace_binding_id"
                @click="transition(data.workspace_binding_id, 'archive')"
              />
              <Button
                v-else
                label="Restore"
                size="small"
                severity="secondary"
                :loading="busyId === data.workspace_binding_id"
                @click="transition(data.workspace_binding_id, 'restore')"
              />
              <Button
                label="Reset"
                size="small"
                severity="danger"
                :loading="busyId === data.workspace_binding_id"
                @click="transition(data.workspace_binding_id, 'reset')"
              />
            </div>
          </template>
        </Column>
      </DataTable>
    </div>
  </section>
</template>
