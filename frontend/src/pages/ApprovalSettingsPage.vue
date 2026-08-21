<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  getAdminApprovalSettings,
  testAdminApprovalSettings,
  updateAdminApprovalSettings,
} from '../api/users'
import { useNotify } from '../composables/useNotify'
import { formatDateTime } from '../utils/format'
import type {
  ApprovalSettingsResponse,
  ApprovalTestResponse,
} from '../generated/openapi/types.gen'

const { success, error: notifyError } = useNotify()
const { t } = useI18n()

const settings = ref<ApprovalSettingsResponse | null>(null)
const enabled = ref(true)
const endpoint = ref('')
const model = ref('')
const apiKey = ref('')
const clearApiKey = ref(false)
const timeoutMs = ref<number | string>(10000)
const maxInputBytes = ref<number | string>(131072)
const maxConcurrent = ref<number | string>(8)
const maxOutputTokens = ref<number | string>(1024)
const loading = ref(false)
const saving = ref(false)
const testing = ref(false)
const error = ref('')
const testResult = ref<ApprovalTestResponse | null>(null)

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  testResult.value = null
  try {
    settings.value = await getAdminApprovalSettings()
    enabled.value = settings.value.enabled
    endpoint.value = settings.value.endpoint ?? ''
    model.value = settings.value.model ?? ''
    apiKey.value = ''
    clearApiKey.value = false
    timeoutMs.value = settings.value.timeout_ms
    maxInputBytes.value = settings.value.max_input_bytes
    maxConcurrent.value = settings.value.max_concurrent
    maxOutputTokens.value = settings.value.max_output_tokens
  } catch (err) {
    error.value =
      err instanceof Error && err.message
        ? err.message
        : t('approval.errors.load')
  } finally {
    loading.value = false
  }
}

function toNumber(
  value: number | string | null | undefined,
  fallback: number,
): number {
  if (value === '' || value === null || value === undefined) return fallback
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : fallback
}

async function save(): Promise<void> {
  saving.value = true
  error.value = ''
  testResult.value = null
  try {
    settings.value = await updateAdminApprovalSettings({
      enabled: enabled.value,
      endpoint: endpoint.value.trim() || null,
      model: model.value.trim() || null,
      api_key: apiKey.value.trim() || null,
      clear_api_key: clearApiKey.value,
      timeout_ms: toNumber(timeoutMs.value, 10000),
      max_input_bytes: toNumber(maxInputBytes.value, 131072),
      max_concurrent: toNumber(maxConcurrent.value, 8),
      max_output_tokens: toNumber(maxOutputTokens.value, 1024),
    })
    enabled.value = settings.value.enabled
    timeoutMs.value = settings.value.timeout_ms
    maxInputBytes.value = settings.value.max_input_bytes
    maxConcurrent.value = settings.value.max_concurrent
    maxOutputTokens.value = settings.value.max_output_tokens
    apiKey.value = ''
    clearApiKey.value = false
    success(t('approval.notifications.saved'))
  } catch (err) {
    error.value =
      err instanceof Error && err.message
        ? err.message
        : t('approval.errors.update')
    notifyError(
      t('approval.notifications.saveFailed'),
      err instanceof Error && err.message ? err.message : undefined,
    )
  } finally {
    saving.value = false
  }
}

async function test(): Promise<void> {
  testing.value = true
  error.value = ''
  try {
    testResult.value = await testAdminApprovalSettings()
  } catch (err) {
    error.value =
      err instanceof Error && err.message
        ? err.message
        : t('approval.errors.test')
  } finally {
    testing.value = false
  }
}

function clearStoredKey(): void {
  apiKey.value = ''
  clearApiKey.value = true
}

onMounted(() => void load())
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
          :aria-label="t('approval.refresh')"
          @click="load"
        />
        <UButton
          icon="i-lucide-plug-zap"
          variant="outline"
          color="neutral"
          :loading="testing"
          :disabled="!enabled"
          @click="test"
        >
          {{ t('approval.testReviewer') }}
        </UButton>
        <UButton
          type="submit"
          form="approval-settings-form"
          icon="i-lucide-save"
          :loading="saving"
          :disabled="saving"
        >
          {{ t('approval.saveSettings') }}
        </UButton>
      </template>
    </PageHeader>

    <div class="grid gap-4 sm:grid-cols-3">
      <div
        class="rounded-xl border border-(--ui-border) bg-(--ui-bg) p-4 shadow-sm"
      >
        <div
          class="text-xs font-medium uppercase tracking-wide text-(--ui-text-muted)"
        >
          {{ t('approval.configured') }}
        </div>
        <div
          class="mt-2 flex items-center gap-2 text-lg font-semibold text-(--ui-text-highlighted)"
        >
          <UIcon
            :name="
              settings?.configured
                ? 'i-lucide-circle-check'
                : 'i-lucide-circle-x'
            "
            :class="
              settings?.configured
                ? 'text-(--ui-text-success)'
                : 'text-(--ui-text-error)'
            "
            class="size-5"
          />
          {{ settings?.configured ? t('approval.yes') : t('approval.no') }}
        </div>
        <p
          v-if="settings?.updated_at"
          class="mt-1 text-xs text-(--ui-text-dimmed)"
        >
          {{
            t('approval.updated', { time: formatDateTime(settings.updated_at) })
          }}
        </p>
      </div>
      <div
        class="rounded-xl border border-(--ui-border) bg-(--ui-bg) p-4 shadow-sm"
      >
        <div
          class="text-xs font-medium uppercase tracking-wide text-(--ui-text-muted)"
        >
          {{ t('approval.apiKey') }}
        </div>
        <div
          class="mt-2 flex items-center gap-2 text-lg font-semibold text-(--ui-text-highlighted)"
        >
          <UIcon
            :name="
              settings?.api_key_configured
                ? 'i-lucide-circle-check'
                : 'i-lucide-circle-x'
            "
            :class="
              settings?.api_key_configured
                ? 'text-(--ui-text-success)'
                : 'text-(--ui-text-error)'
            "
            class="size-5"
          />
          {{
            settings?.api_key_configured
              ? t('approval.configured')
              : t('approval.missing')
          }}
        </div>
        <p class="mt-1 text-xs text-(--ui-text-dimmed)">
          {{
            t('approval.source', {
              source:
                settings?.api_key_source === 'database'
                  ? t('approval.apiKeySources.database')
                  : settings?.api_key_source === 'environment'
                    ? t('approval.apiKeySources.environment')
                    : t('approval.apiKeySources.none'),
            })
          }}
        </p>
      </div>
      <div
        class="rounded-xl border border-(--ui-border) bg-(--ui-bg) p-4 shadow-sm"
      >
        <div
          class="text-xs font-medium uppercase tracking-wide text-(--ui-text-muted)"
        >
          {{ t('approval.mode') }}
        </div>
        <div
          class="mt-2 flex items-center gap-2 text-lg font-semibold text-(--ui-text-highlighted)"
        >
          <UIcon
            :name="enabled ? 'i-lucide-bot' : 'i-lucide-bot-off'"
            class="size-5 text-(--ui-text-dimmed)"
          />
          {{ enabled ? t('approval.enabled') : t('approval.disabled') }}
        </div>
        <p class="mt-1 text-xs text-(--ui-text-dimmed)">
          {{ t('approval.inheritedDescription') }}
        </p>
      </div>
    </div>

    <section
      class="rounded-xl border border-(--ui-border) bg-(--ui-bg) p-5 shadow-sm"
    >
      <ErrorAlert v-if="error" :error="error" class="mb-4" @retry="load" />
      <form
        id="approval-settings-form"
        class="grid gap-4 md:grid-cols-2"
        @submit.prevent="save"
      >
        <div
          class="flex items-center justify-between rounded-lg border border-(--ui-border) p-3 md:col-span-2"
        >
          <div>
            <div class="text-sm font-medium text-(--ui-text-highlighted)">
              {{ t('approval.enableAutomaticReview') }}
            </div>
            <div class="text-xs text-(--ui-text-muted)">
              {{ t('approval.automaticReviewDescription') }}
            </div>
          </div>
          <USwitch v-model="enabled" />
        </div>
        <UFormField
          :label="t('approval.endpoint')"
          class="md:col-span-2"
          :hint="t('approval.endpointHint')"
        >
          <UInput
            v-model="endpoint"
            :placeholder="t('approval.endpointPlaceholder')"
          />
        </UFormField>
        <UFormField :label="t('approval.model')">
          <UInput
            v-model="model"
            :placeholder="t('approval.modelPlaceholder')"
          />
        </UFormField>
        <UFormField
          :label="t('approval.apiKey')"
          :hint="t('approval.apiKeyHint')"
          class="md:col-span-2"
        >
          <div class="flex gap-2">
            <UInput
              v-model="apiKey"
              type="password"
              autocomplete="new-password"
              :placeholder="t('approval.apiKeyPlaceholder')"
              class="min-w-0 flex-1"
              @input="clearApiKey = false"
            />
            <UButton
              v-if="settings?.api_key_source === 'database'"
              type="button"
              icon="i-lucide-trash-2"
              variant="outline"
              color="error"
              :aria-label="t('approval.clearStoredApiKey')"
              @click="clearStoredKey"
            />
          </div>
        </UFormField>
        <UFormField :label="t('approval.timeout')">
          <UInput
            v-model.number="timeoutMs"
            type="number"
            min="100"
            max="30000"
          />
        </UFormField>
        <UFormField :label="t('approval.maxInput')">
          <UInput
            v-model.number="maxInputBytes"
            type="number"
            min="1"
            max="524288"
          />
        </UFormField>
        <UFormField :label="t('approval.concurrentReviews')">
          <UInput
            v-model.number="maxConcurrent"
            type="number"
            min="1"
            max="64"
          />
        </UFormField>
        <UFormField :label="t('approval.maxOutputTokens')">
          <UInput
            v-model.number="maxOutputTokens"
            type="number"
            min="256"
            max="8192"
          />
        </UFormField>
      </form>
      <UAlert
        v-if="testResult"
        class="mt-4"
        :title="
          testResult.ok ? t('approval.testPassed') : t('approval.testFailed')
        "
        :description="
          t('approval.testDescription', {
            message: testResult.message,
            latency: testResult.latency_ms,
          })
        "
        :color="testResult.ok ? 'success' : 'error'"
        variant="subtle"
      />
    </section>
  </div>
</template>
