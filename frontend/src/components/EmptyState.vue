<script setup lang="ts">
import { useI18n } from 'vue-i18n'

const props = defineProps<{
  title?: string
  description?: string
}>()

const { t } = useI18n()
</script>

<template>
  <div
    class="flex flex-col items-center justify-center gap-2 px-6 py-16 text-center"
  >
    <UIcon name="i-lucide-inbox" class="size-8 text-(--ui-text-dimmed)" />
    <p class="text-sm font-medium text-(--ui-text-muted)">
      {{ props.title ?? t('shared.empty.noData') }}
    </p>
    <p
      v-if="props.description"
      class="max-w-sm text-xs text-(--ui-text-dimmed)"
    >
      {{ props.description }}
    </p>
    <div v-if="$slots.actions" class="mt-2">
      <slot name="actions" />
    </div>
  </div>
</template>
