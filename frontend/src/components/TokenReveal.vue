<script setup lang="ts">
import { useNotify } from '../composables/useNotify'

defineProps<{
  token: string
}>()

const { copy } = useNotify()
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
      :aria-label="'Copy token'"
      @click="copy(token, 'Token copied')"
    />
  </div>
</template>
