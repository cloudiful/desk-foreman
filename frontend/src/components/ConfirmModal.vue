<script setup lang="ts">
const props = defineProps<{
  title: string
  description?: string
  confirmLabel?: string
  confirmColor?: 'primary' | 'error' | 'warning' | 'success' | 'neutral'
  loading?: boolean
}>()

const emit = defineEmits<{
  confirm: []
}>()

const open = defineModel<boolean>('open', { required: true })
</script>

<template>
  <UModal
    v-model:open="open"
    :title="props.title"
    :description="props.description"
  >
    <template #body>
      <slot />
    </template>
    <template #footer>
      <UButton
        variant="outline"
        @click="
          () => {
            open = false
          }
        "
      >
        Cancel
      </UButton>
      <UButton
        :color="props.confirmColor ?? 'primary'"
        :loading="props.loading"
        @click="emit('confirm')"
      >
        {{ props.confirmLabel ?? 'Confirm' }}
      </UButton>
    </template>
  </UModal>
</template>
