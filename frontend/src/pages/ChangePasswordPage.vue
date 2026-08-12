<script setup lang="ts">
import Button from 'primevue/button'
import Card from 'primevue/card'
import Password from 'primevue/password'
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { authState } from '../api/auth'

const router = useRouter()
const currentPassword = ref('')
const newPassword = ref('')
const confirmPassword = ref('')
const error = ref('')
const loading = ref(false)

async function submit(): Promise<void> {
  error.value = ''
  if (newPassword.value.length < 8) {
    error.value = 'Password must be at least 8 characters'
    return
  }
  if (newPassword.value !== confirmPassword.value) {
    error.value = 'Passwords do not match'
    return
  }
  loading.value = true
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
  <div class="flex min-h-screen items-center justify-center px-4">
    <Card class="app-shell-panel w-full max-w-md rounded-[2rem]">
      <template #title>
        <div class="text-xs uppercase tracking-[0.3em] text-[var(--muted)]">
          Desk Foreman
        </div>
        <div class="mt-2 text-3xl font-semibold">Set a new password</div>
      </template>
      <template #content>
        <form class="space-y-4" @submit.prevent="submit">
          <Password
            v-model="currentPassword"
            class="w-full"
            fluid
            :feedback="false"
            placeholder="Current password"
          />
          <Password
            v-model="newPassword"
            class="w-full"
            fluid
            toggle-mask
            placeholder="New password"
          />
          <Password
            v-model="confirmPassword"
            class="w-full"
            fluid
            :feedback="false"
            placeholder="Confirm new password"
          />
          <p v-if="error" class="text-sm text-red-700">{{ error }}</p>
          <Button
            type="submit"
            label="Change password"
            class="w-full"
            :loading="loading"
          />
        </form>
      </template>
    </Card>
  </div>
</template>
