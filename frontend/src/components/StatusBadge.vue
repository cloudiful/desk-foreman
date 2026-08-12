<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{
  status: string
}>()

const config = computed(() => {
  switch (props.status) {
    case 'online':
    case 'active':
    case 'success':
    case 'running':
      return { color: 'success', dot: true } as const
    case 'offline':
    case 'failed':
    case 'error':
    case 'inactive':
      return { color: 'error', dot: true } as const
    case 'pending':
    case 'idle':
    case 'archived':
      return { color: 'warning', dot: true } as const
    case 'disabled':
      return { color: 'neutral', dot: false } as const
    default:
      return { color: 'neutral', dot: false } as const
  }
})
</script>

<template>
  <UBadge :color="config.color" variant="subtle" size="sm">
    <template #leading>
      <span v-if="config.dot" class="size-1.5 rounded-full bg-current" />
    </template>
    {{ status }}
  </UBadge>
</template>
