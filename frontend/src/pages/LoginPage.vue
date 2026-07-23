<script setup lang="ts">
import Button from 'primevue/button'
import Card from 'primevue/card'
import InputText from 'primevue/inputtext'
import Password from 'primevue/password'
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { authState } from '../api/auth'

const router = useRouter()
const loginName = ref('')
const password = ref('')
const loading = ref(false)
const error = ref('')

async function submit(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    await authState.loginWithPassword({
      login_name: loginName.value,
      password: password.value,
    })
    await router.push('/')
  } catch (err) {
    error.value = err instanceof Error ? err.message : 'Login failed'
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
        <div class="mt-2 text-3xl font-semibold">Sign in</div>
      </template>
      <template #content>
        <form class="space-y-4" @submit.prevent="submit">
          <div class="space-y-2">
            <label class="block text-sm font-medium">Login name</label>
            <InputText
              v-model="loginName"
              class="w-full"
              autocomplete="username"
            />
          </div>
          <div class="space-y-2">
            <label class="block text-sm font-medium">Password</label>
            <Password
              v-model="password"
              class="w-full"
              fluid
              :feedback="false"
              toggle-mask
            />
          </div>
          <p v-if="error" class="text-sm text-red-700">{{ error }}</p>
          <Button
            type="submit"
            label="Login"
            class="w-full"
            :loading="loading"
          />
        </form>
      </template>
    </Card>
  </div>
</template>
