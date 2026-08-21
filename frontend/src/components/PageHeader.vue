<script setup lang="ts">
import { useI18n } from 'vue-i18n'

defineProps<{
  title: string
  description?: string
}>()

const { t } = useI18n()
</script>

<template>
  <div
    class="flex flex-col gap-4 md:flex-row md:items-center md:justify-between"
  >
    <div class="min-w-0">
      <div class="flex items-center gap-2 text-sm text-(--ui-text-muted)">
        <UBreadcrumb
          :items="[
            { label: t('shared.pageHeader.controlPlane') },
            { label: title },
          ]"
        />
      </div>
      <h1
        class="mt-1 text-2xl font-semibold tracking-tight text-(--ui-text-highlighted)"
      >
        {{ title }}
      </h1>
      <p v-if="description" class="mt-1 text-sm text-(--ui-text-muted)">
        {{ description }}
      </p>
    </div>
    <div class="flex shrink-0 flex-wrap items-center gap-2">
      <slot name="actions" />
    </div>
  </div>
</template>
