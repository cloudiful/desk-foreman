<script setup lang="ts">
import Column from 'primevue/column'
import DataTable from 'primevue/datatable'
import Tab from 'primevue/tab'
import TabList from 'primevue/tablist'
import TabPanel from 'primevue/tabpanel'
import TabPanels from 'primevue/tabpanels'
import Tabs from 'primevue/tabs'
import { onMounted, ref } from 'vue'
import {
  listAdminRunnerSessions,
  listAdminWorkspaceRunners,
} from '../api/users'
import type {
  RunnerSessionResponse,
  WorkspaceRunnerResponse,
} from '../generated/openapi/types.gen'

const runners = ref<WorkspaceRunnerResponse[]>([])
const sessions = ref<RunnerSessionResponse[]>([])
const loading = ref(false)
const error = ref('')

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    ;[runners.value, sessions.value] = await Promise.all([
      listAdminWorkspaceRunners(),
      listAdminRunnerSessions(),
    ])
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to load runner status'
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
      <h2 class="mt-2 text-2xl font-semibold">Runners and sessions</h2>
      <p v-if="error" class="mt-3 text-sm text-red-700">{{ error }}</p>
    </div>
    <div class="app-shell-panel overflow-hidden rounded-[2rem] p-3">
      <Tabs value="0">
        <TabList>
          <Tab value="0">Runners</Tab>
          <Tab value="1">Sessions</Tab>
        </TabList>
        <TabPanels>
          <TabPanel value="0">
            <DataTable :value="runners" :loading="loading" striped-rows>
              <Column field="runner_id" header="Runner" />
              <Column field="owner_kind" header="Owner" />
              <Column field="status" header="Status" />
              <Column field="runtime" header="Runtime" />
              <Column field="last_active_at" header="Last active" />
              <Column field="last_error" header="Last error" />
            </DataTable>
          </TabPanel>
          <TabPanel value="1">
            <DataTable :value="sessions" :loading="loading" striped-rows>
              <Column field="session_id" header="Session" />
              <Column field="owner_kind" header="Owner" />
              <Column field="owner_id" header="Owner ID" />
              <Column field="state" header="State" />
              <Column field="exit_code" header="Exit code" />
              <Column field="wall_time_seconds" header="Duration" />
            </DataTable>
          </TabPanel>
        </TabPanels>
      </Tabs>
    </div>
  </section>
</template>
