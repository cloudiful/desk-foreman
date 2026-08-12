<script setup lang="ts">
import Button from 'primevue/button'
import Dialog from 'primevue/dialog'
import InputNumber from 'primevue/inputnumber'
import InputSwitch from 'primevue/inputswitch'
import InputText from 'primevue/inputtext'
import DataTable from 'primevue/datatable'
import Column from 'primevue/column'
import { onMounted, ref } from 'vue'
import {
  createAdminRunnerManager,
  listAdminRunnerManagers,
  updateAdminRunnerManager,
} from '../api/users'
import type {
  CreateRunnerManagerRequest,
  RunnerManagerResponse,
} from '../generated/openapi/types.gen'

const rows = ref<RunnerManagerResponse[]>([])
const loading = ref(false)
const error = ref('')
const dialogVisible = ref(false)
const tokenVisible = ref(false)
const token = ref('')
const editingId = ref<number | null>(null)
const enabled = ref(true)
const form = ref<CreateRunnerManagerRequest>({
  name: '',
  endpoint: 'http://desk-foreman-runner-manager:3001',
  access_token: undefined,
  image: 'desk-foreman-workspace-runner:local',
  network_enabled: false,
  max_output_bytes: 262144,
  max_timeout_ms: 600000,
  max_sessions: 32,
  pids_limit: 256,
  memory_limit: '1g',
  cpu_limit: '2',
})

async function load(): Promise<void> {
  loading.value = true
  try {
    rows.value = await listAdminRunnerManagers()
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to load runner managers'
  } finally {
    loading.value = false
  }
}

function openCreate(): void {
  editingId.value = null
  enabled.value = true
  form.value = {
    name: '',
    endpoint: 'http://desk-foreman-runner-manager:3001',
    access_token: undefined,
    image: 'desk-foreman-workspace-runner:local',
    network_enabled: false,
    max_output_bytes: 262144,
    max_timeout_ms: 600000,
    max_sessions: 32,
    pids_limit: 256,
    memory_limit: '1g',
    cpu_limit: '2',
  }
  dialogVisible.value = true
}

function openEdit(row: RunnerManagerResponse): void {
  editingId.value = row.runner_manager_id
  enabled.value = row.enabled
  form.value = {
    name: row.name,
    endpoint: row.endpoint,
    access_token: undefined,
    image: row.image,
    network_enabled: row.network_enabled,
    max_output_bytes: row.max_output_bytes,
    max_timeout_ms: row.max_timeout_ms,
    max_sessions: row.max_sessions,
    pids_limit: row.pids_limit,
    memory_limit: row.memory_limit,
    cpu_limit: row.cpu_limit,
  }
  dialogVisible.value = true
}

async function save(): Promise<void> {
  error.value = ''
  try {
    if (editingId.value === null) {
      const result = await createAdminRunnerManager(form.value)
      if (result.token) {
        token.value = result.token
        tokenVisible.value = true
      }
    } else {
      await updateAdminRunnerManager(editingId.value, {
        endpoint: form.value.endpoint,
        enabled: enabled.value,
        image: form.value.image,
        network_enabled: form.value.network_enabled,
        max_output_bytes: form.value.max_output_bytes,
        max_timeout_ms: form.value.max_timeout_ms,
        max_sessions: form.value.max_sessions,
        pids_limit: form.value.pids_limit,
        memory_limit: form.value.memory_limit,
        cpu_limit: form.value.cpu_limit,
      })
    }
    dialogVisible.value = false
    await load()
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to save runner manager'
  }
}

onMounted(() => void load())
</script>

<template>
  <section class="space-y-4">
    <div class="app-shell-panel rounded-[2rem] p-5">
      <div class="flex items-center justify-between gap-3">
        <div>
          <div class="text-xs uppercase tracking-[0.25em] text-[var(--muted)]">
            Admin
          </div>
          <h2 class="mt-2 text-2xl font-semibold">Runner managers</h2>
        </div>
        <Button label="Add runner manager" @click="openCreate" />
      </div>
      <p v-if="error" class="mt-3 text-sm text-red-700">{{ error }}</p>
    </div>
    <div class="app-shell-panel overflow-hidden rounded-[2rem] p-3">
      <DataTable :value="rows" :loading="loading" striped-rows>
        <Column field="name" header="Name" />
        <Column field="endpoint" header="Endpoint" />
        <Column field="status" header="Status" />
        <Column field="image" header="Image" />
        <Column field="last_seen_at" header="Last seen" />
        <Column header="Actions">
          <template #body="{ data }">
            <Button label="Edit" severity="secondary" @click="openEdit(data)" />
          </template>
        </Column>
      </DataTable>
    </div>
    <Dialog
      v-model:visible="dialogVisible"
      modal
      header="Runner manager"
      :style="{ width: '32rem' }"
    >
      <form class="space-y-3" @submit.prevent="save">
        <InputText
          v-model="form.name"
          class="w-full"
          placeholder="Name"
          :disabled="editingId !== null"
        />
        <InputText
          v-model="form.endpoint"
          class="w-full"
          placeholder="http://runner-manager:3001"
        />
        <InputText
          v-if="editingId === null"
          v-model="form.access_token"
          class="w-full"
          placeholder="Existing runner token (optional)"
          type="password"
        />
        <InputText
          v-model="form.image"
          class="w-full"
          placeholder="Runner image"
        />
        <div class="flex items-center justify-between">
          <span>Enabled</span><InputSwitch v-model="enabled" />
        </div>
        <div class="flex items-center justify-between">
          <span>Network enabled</span
          ><InputSwitch v-model="form.network_enabled" />
        </div>
        <InputNumber
          v-model="form.max_output_bytes"
          class="w-full"
          input-class="w-full"
          placeholder="Max output bytes"
        />
        <InputNumber
          v-model="form.max_timeout_ms"
          class="w-full"
          input-class="w-full"
          placeholder="Max timeout ms"
        />
        <InputNumber
          v-model="form.max_sessions"
          class="w-full"
          input-class="w-full"
          placeholder="Max sessions"
        />
        <InputNumber
          v-model="form.pids_limit"
          class="w-full"
          input-class="w-full"
          placeholder="PID limit"
        />
        <InputText
          v-model="form.memory_limit"
          class="w-full"
          placeholder="Memory limit"
        />
        <InputText
          v-model="form.cpu_limit"
          class="w-full"
          placeholder="CPU limit"
        />
        <Button type="submit" label="Save" class="w-full" />
      </form>
    </Dialog>
    <Dialog
      v-model:visible="tokenVisible"
      modal
      header="Runner token"
      :style="{ width: '32rem' }"
    >
      <p class="text-sm">
        Copy this token into the runner-manager deployment. It is shown only
        once.
      </p>
      <InputText :model-value="token" readonly class="mt-3 w-full" />
    </Dialog>
  </section>
</template>
