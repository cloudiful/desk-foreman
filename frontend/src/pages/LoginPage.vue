<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { authState } from '../api/auth'

const router = useRouter()

const loginName = ref('')
const password = ref('')
const loading = ref(false)
const error = ref('')

async function submit(): Promise<void> {
  if (!loginName.value.trim() || !password.value) {
    error.value = 'Enter your login name and password'
    return
  }
  loading.value = true
  error.value = ''
  try {
    await authState.loginWithPassword({
      login_name: loginName.value.trim(),
      password: password.value,
    })
    await router.push('/')
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Login failed'
    password.value = ''
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
          <UIcon name="i-lucide-hammer" class="size-6" />
        </div>
        <div class="text-center">
          <h1
            class="text-xl font-semibold tracking-tight text-(--ui-text-highlighted)"
          >
            Desk Foreman
          </h1>
          <p class="mt-1 text-sm text-(--ui-text-muted)">
            Sign in to the control plane
          </p>
        </div>
      </div>

      <form
        class="space-y-4 rounded-xl border border-(--ui-border) bg-(--ui-bg) p-6 shadow-sm"
        @submit.prevent="submit"
      >
        <UFormField label="Login name">
          <UInput
            v-model="loginName"
            name="login_name"
            autocomplete="username"
            size="lg"
            placeholder="admin"
            leading-icon="i-lucide-user"
          />
        </UFormField>
        <UFormField label="Password">
          <UInput
            v-model="password"
            name="password"
            type="password"
            autocomplete="current-password"
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
          leading-icon="i-lucide-log-in"
        >
          Sign in
        </UButton>
      </form>
    </div>
  </div>
</template>
