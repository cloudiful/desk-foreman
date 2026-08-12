<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { authState } from '../api/auth'

const router = useRouter()
const currentPassword = ref('')
const newPassword = ref('')
const confirmPassword = ref('')
const error = ref('')
const loading = ref(false)

function validate(): string | null {
  if (!currentPassword.value) return 'Enter your current password'
  if (newPassword.value.length < 8)
    return 'New password must be at least 8 characters'
  if (newPassword.value !== confirmPassword.value)
    return 'Passwords do not match'
  return null
}

async function submit(): Promise<void> {
  const problem = validate()
  if (problem) {
    error.value = problem
    return
  }
  loading.value = true
  error.value = ''
  try {
    await authState.changeCurrentPassword(
      currentPassword.value,
      newPassword.value,
    )
    await router.push('/')
  } catch (err) {
    error.value =
      err instanceof Error ? err.message : 'Failed to change password'
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <div
    class="flex min-h-screen items-center justify-center bg-(--ui-bg-muted) px-4"
  >
    <div class="w-full max-w-sm">
      <div class="mb-8 flex flex-col items-center gap-3">
        <div
          class="flex size-12 items-center justify-center rounded-xl bg-(--ui-primary) text-white shadow-lg shadow-(--ui-primary)/20"
        >
          <UIcon name="i-lucide-key-round" class="size-6" />
        </div>
        <div class="text-center">
          <h1
            class="text-xl font-semibold tracking-tight text-(--ui-text-highlighted)"
          >
            Set a new password
          </h1>
          <p class="mt-1 text-sm text-(--ui-text-muted)">
            You must change the default password before continuing
          </p>
        </div>
      </div>

      <form
        class="space-y-4 rounded-xl border border-(--ui-border) bg-(--ui-bg) p-6 shadow-sm"
        @submit.prevent="submit"
      >
        <UFormField label="Current password">
          <UInput
            v-model="currentPassword"
            name="current_password"
            type="password"
            autocomplete="current-password"
            size="lg"
            placeholder="••••••••"
            leading-icon="i-lucide-lock"
          />
        </UFormField>
        <UFormField label="New password" hint="At least 8 characters">
          <UInput
            v-model="newPassword"
            name="new_password"
            type="password"
            autocomplete="new-password"
            size="lg"
            placeholder="••••••••"
            leading-icon="i-lucide-lock"
          />
        </UFormField>
        <UFormField label="Confirm new password">
          <UInput
            v-model="confirmPassword"
            name="confirm_password"
            type="password"
            autocomplete="new-password"
            size="lg"
            placeholder="••••••••"
            leading-icon="i-lucide-lock"
          />
        </UFormField>

        <UAlert
          v-if="error"
          :title="error"
          color="error"
          variant="subtle"
          icon="i-lucide-circle-alert"
        />

        <UButton
          type="submit"
          size="lg"
          block
          :loading="loading"
          leading-icon="i-lucide-check"
        >
          Change password
        </UButton>
      </form>
    </div>
  </div>
</template>
