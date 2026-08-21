<script setup lang="ts">
import { onErrorCaptured, ref } from 'vue'
import { RouterView } from 'vue-router'
import { useI18n } from 'vue-i18n'

const renderError = ref(false)
const { t } = useI18n()

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
    <div
      v-if="renderError"
      class="flex min-h-screen items-center justify-center p-6"
    >
      <UAlert
        :title="t('app.errorBoundary.title')"
        :description="t('app.errorBoundary.description')"
        color="error"
        variant="subtle"
      >
        <template #actions>
          <UButton color="error" variant="soft" @click="reload">
            {{ t('app.errorBoundary.reload') }}
          </UButton>
        </template>
      </UAlert>
    </div>
    <RouterView v-else />
  </UApp>
</template>
