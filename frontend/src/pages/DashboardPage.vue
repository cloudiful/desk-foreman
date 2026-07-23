<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { getAdminOperationsSummary } from '../api/users'
import type { OperationsSummary } from '../generated/openapi/types.gen'

const summary = ref<OperationsSummary | null>(null)
const error = ref('')

onMounted(async () => {
  try {
    summary.value = await getAdminOperationsSummary()
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to load operations summary'
  }
})
</script>

<template>
  <section class="space-y-4">
    <div
      v-if="error"
      class="app-shell-panel rounded-[2rem] p-5 text-sm text-red-700"
    >
      {{ error }}
    </div>
    <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
      <article
        v-for="item in [
          ['Active runners', summary?.active_runners ?? 0],
          ['Active sessions', summary?.active_sessions ?? 0],
          ['Failed operations', summary?.failed_operations ?? 0],
          ['Archived workspaces', summary?.archived_workspaces ?? 0],
        ]"
        :key="item[0]"
        class="app-shell-panel rounded-[2rem] p-5"
      >
        <div class="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
          {{ item[0] }}
        </div>
        <div class="mt-3 text-3xl font-semibold">{{ item[1] }}</div>
      </article>
    </div>
    <div class="grid gap-4 md:grid-cols-2">
      <article class="app-shell-panel rounded-[2rem] p-6">
        <div class="text-xs uppercase tracking-[0.25em] text-[var(--muted)]">
          Status
        </div>
        <h2 class="mt-2 text-2xl font-semibold">Control plane ready</h2>
        <p class="mt-3 text-sm leading-6 text-[var(--muted)]">
          This console manages authenticated web access and
          administrator-controlled user lifecycle for Desk Foreman.
        </p>
      </article>
      <article class="app-shell-panel rounded-[2rem] p-6">
        <div class="text-xs uppercase tracking-[0.25em] text-[var(--muted)]">
          Scope
        </div>
        <h2 class="mt-2 text-2xl font-semibold">Users, sessions, MCP tokens</h2>
        <p class="mt-3 text-sm leading-6 text-[var(--muted)]">
          MCP bearer access remains separate from browser login state. The web
          surface is the admin entry point for user lifecycle operations.
        </p>
      </article>
    </div>
  </section>
</template>
