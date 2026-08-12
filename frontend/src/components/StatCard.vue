<script setup lang="ts">
defineProps<{
  title: string
  value: string | number
  icon: string
  hint?: string
  loading?: boolean
}>()
</script>

<template>
  <div
    class="rounded-lg border border-(--ui-border) bg-(--ui-bg) p-4 shadow-sm"
  >
    <div class="flex items-center justify-between">
      <p
        class="text-xs font-medium uppercase tracking-wide text-(--ui-text-muted)"
      >
        {{ title }}
      </p>
      <UIcon :name="icon" class="size-4 text-(--ui-text-dimmed)" />
    </div>
    <div
      v-if="loading"
      class="mt-3 h-8 w-20 animate-pulse rounded bg-(--ui-bg-elevated)"
    />
    <p
      v-else
      class="mt-2 text-3xl font-semibold tabular-nums text-(--ui-text-highlighted)"
    >
      {{ value }}
    </p>
    <p v-if="hint" class="mt-1 text-xs text-(--ui-text-dimmed)">
      {{ hint }}
    </p>
  </div>
</template>
