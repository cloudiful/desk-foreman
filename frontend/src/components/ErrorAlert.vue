<script setup lang="ts">
defineProps<{
  error: string
}>()

defineEmits<{
  retry: []
}>()
</script>

<template>
  <UAlert
    :title="error"
    description="The request failed. Check the details and try again."
    color="error"
    variant="subtle"
  >
    <template #actions>
      <slot name="retry">
        <UButton color="error" variant="soft" size="xs" @click="$emit('retry')">
          Retry
        </UButton>
      </slot>
    </template>
  </UAlert>
</template>
