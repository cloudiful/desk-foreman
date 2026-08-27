<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

const props = defineProps<{
  status: string
}>()

const { t, te } = useI18n()

const config = computed(() => {
  switch (props.status) {
    case 'online':
    case 'active':
    case 'success':
    case 'running':
      return { color: 'success', dot: true } as const
    case 'offline':
    case 'failed':
    case 'cleanup_failed':
    case 'error':
    case 'inactive':
      return { color: 'error', dot: true } as const
    case 'pending':
    case 'idle':
    case 'stale':
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
    <span class="whitespace-nowrap">
      {{
        status
          ? te(`shared.status.${status}`)
            ? t(`shared.status.${status}`)
            : status
          : t('shared.status.unknown')
      }}
    </span>
  </UBadge>
</template>
