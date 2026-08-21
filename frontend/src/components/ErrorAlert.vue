<script setup lang="ts">
import { useI18n } from 'vue-i18n'

defineProps<{
  error: string
}>()

defineEmits<{
  retry: []
}>()

const { t } = useI18n()
</script>

<template>
  <UAlert
    :title="error"
    :description="t('shared.errorAlert.description')"
    color="error"
    variant="subtle"
  >
    <template #actions>
      <slot name="retry">
        <UButton color="error" variant="soft" size="xs" @click="$emit('retry')">
          {{ t('shared.errorAlert.retry') }}
        </UButton>
      </slot>
    </template>
  </UAlert>
</template>
