<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { useNotify } from '../composables/useNotify'

defineProps<{
  token: string
}>()

const { copy } = useNotify()
const { t } = useI18n()
</script>

<template>
  <div
    class="flex items-center gap-2 rounded-md border border-(--ui-border-accented) bg-(--ui-bg-elevated) px-3 py-2.5"
  >
    <code
      class="min-w-0 flex-1 select-all break-all font-mono text-xs text-(--ui-text-highlighted)"
    >
      {{ token }}
    </code>
    <UButton
      icon="i-lucide-copy"
      size="xs"
      variant="outline"
      color="neutral"
      :aria-label="t('shared.token.copyAriaLabel')"
      @click="copy(token, t('shared.token.copied'))"
    />
  </div>
</template>
