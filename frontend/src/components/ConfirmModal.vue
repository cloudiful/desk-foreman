<script setup lang="ts">
import { useI18n } from 'vue-i18n'

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
const { t } = useI18n()
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
        {{ t('shared.confirm.cancel') }}
      </UButton>
      <UButton
        :color="props.confirmColor ?? 'primary'"
        :loading="props.loading"
        :disabled="props.loading"
        type="button"
        @click="emit('confirm')"
      >
        {{ props.confirmLabel ?? t('shared.confirm.confirm') }}
      </UButton>
    </template>
  </UModal>
</template>
