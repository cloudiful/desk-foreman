<script setup lang="ts">
defineProps<{
  title?: string
  description?: string
}>()
</script>

<template>
  <div class="flex min-h-9 items-center justify-end gap-2">
    <slot name="actions" />
  </div>
</template>
