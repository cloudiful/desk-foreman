<script setup lang="ts">
import Button from 'primevue/button'
import InputNumber from 'primevue/inputnumber'
import InputText from 'primevue/inputtext'
import { onMounted, ref } from 'vue'
import {
  getAdminApprovalSettings,
  updateAdminApprovalSettings,
} from '../api/users'
import type { ApprovalSettingsResponse } from '../generated/openapi/types.gen'

const settings = ref<ApprovalSettingsResponse | null>(null)
const endpoint = ref('')
const model = ref('')
const timeoutMs = ref(10000)
const maxInputBytes = ref(131072)
const maxConcurrent = ref(8)
const loading = ref(false)
const saving = ref(false)
const error = ref('')
const saved = ref(false)

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    settings.value = await getAdminApprovalSettings()
    endpoint.value = settings.value.endpoint ?? ''
    model.value = settings.value.model ?? ''
    timeoutMs.value = settings.value.timeout_ms
    maxInputBytes.value = settings.value.max_input_bytes
    maxConcurrent.value = settings.value.max_concurrent
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to load approval settings'
  } finally {
    loading.value = false
  }
}

async function save(): Promise<void> {
  saving.value = true
  saved.value = false
  error.value = ''
  try {
    settings.value = await updateAdminApprovalSettings({
      endpoint: endpoint.value.trim() || null,
      model: model.value.trim() || null,
      timeout_ms: timeoutMs.value,
      max_input_bytes: maxInputBytes.value,
      max_concurrent: maxConcurrent.value,
    })
    saved.value = true
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to update approval settings'
  } finally {
    saving.value = false
  }
}

onMounted(() => void load())
</script>

<template>
  <section class="space-y-4">
    <div class="app-shell-panel rounded-[2rem] p-5">
      <div
        class="flex flex-col gap-3 md:flex-row md:items-center md:justify-between"
      >
        <div>
          <div class="text-xs uppercase tracking-[0.25em] text-[var(--muted)]">
            Admin
          </div>
          <h2 class="mt-2 text-2xl font-semibold">Approval reviewer</h2>
        </div>
        <Button label="Save settings" :loading="saving" @click="save" />
      </div>
      <p v-if="error" class="mt-3 text-sm text-red-700">{{ error }}</p>
      <p v-if="saved" class="mt-3 text-sm text-emerald-700">
        Settings updated.
      </p>
    </div>

    <div
      class="app-shell-panel space-y-5 rounded-[2rem] p-5"
      :aria-busy="loading"
    >
      <div class="grid gap-4 md:grid-cols-2">
        <div class="space-y-2 md:col-span-2">
          <label class="block text-sm font-medium"
            >Responses API base URL</label
          >
          <InputText
            v-model="endpoint"
            class="w-full"
            placeholder="https://api.openai.com/v1"
          />
        </div>
        <div class="space-y-2">
          <label class="block text-sm font-medium">Model</label>
          <InputText
            v-model="model"
            class="w-full"
            placeholder="Reviewer model"
          />
        </div>
        <div class="space-y-2">
          <label class="block text-sm font-medium">Timeout (ms)</label>
          <InputNumber
            v-model="timeoutMs"
            class="w-full"
            input-class="w-full"
            :min="100"
            :max="30000"
          />
        </div>
        <div class="space-y-2">
          <label class="block text-sm font-medium">Max input bytes</label>
          <InputNumber
            v-model="maxInputBytes"
            class="w-full"
            input-class="w-full"
            :min="1"
            :max="524288"
          />
        </div>
        <div class="space-y-2">
          <label class="block text-sm font-medium">Concurrent reviews</label>
          <InputNumber
            v-model="maxConcurrent"
            class="w-full"
            input-class="w-full"
            :min="1"
            :max="64"
          />
        </div>
      </div>
      <div class="grid gap-3 text-sm md:grid-cols-3">
        <div class="rounded-2xl bg-black/4 p-4">
          Configured: {{ settings?.configured ? 'Yes' : 'No' }}
        </div>
        <div class="rounded-2xl bg-black/4 p-4">
          API key: {{ settings?.api_key_configured ? 'Available' : 'Missing' }}
        </div>
        <div class="rounded-2xl bg-black/4 p-4">Mode: auto review</div>
      </div>
    </div>
  </section>
</template>
