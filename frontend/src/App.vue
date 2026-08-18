<script setup lang="ts">
import { onErrorCaptured, ref } from 'vue'
import { RouterView } from 'vue-router'

const renderError = ref(false)

onErrorCaptured(() => {
  renderError.value = true
  return false
})

function reload(): void {
  window.location.reload()
}
</script>

<template>
  <UApp>
    <div v-if="renderError" class="flex min-h-screen items-center justify-center p-6">
      <UAlert
        title="The page could not be rendered"
        description="Reload the page to recover the application."
        color="error"
        variant="subtle"
      >
        <template #actions>
          <UButton color="error" variant="soft" @click="reload">
            Reload
          </UButton>
        </template>
      </UAlert>
    </div>
    <RouterView v-else />
  </UApp>
</template>
