<script setup lang="ts">
const props = defineProps<{
  title: string
  description?: string
  confirmLabel?: string
  confirmColor?:
    | 'primary'
    | 'secondary'
    | 'success'
    | 'info'
    | 'warning'
    | 'error'
    | 'neutral'
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
    :dismissible="!props.loading"
    :close="!props.loading"
  >
    <template #body>
      <slot />
    </template>
    <template #footer>
      <UButton
        variant="outline"
        :disabled="props.loading"
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
        :disabled="props.loading"
        type="button"
        @click="emit('confirm')"
      >
        {{ props.confirmLabel ?? 'Confirm' }}
      </UButton>
    </template>
  </UModal>
</template>
