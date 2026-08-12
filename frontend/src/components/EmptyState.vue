<script setup lang="ts">
defineProps<{
  title?: string
  description?: string
}>()
</script>

<template>
  <div
    class="flex flex-col items-center justify-center gap-2 px-6 py-16 text-center"
  >
    <UIcon name="i-lucide-inbox" class="size-8 text-(--ui-text-dimmed)" />
    <p class="text-sm font-medium text-(--ui-text-muted)">
      {{ title ?? 'No data' }}
    </p>
    <p v-if="description" class="max-w-sm text-xs text-(--ui-text-dimmed)">
      {{ description }}
    </p>
    <div v-if="$slots.actions" class="mt-2">
      <slot name="actions" />
    </div>
  </div>
</template>
