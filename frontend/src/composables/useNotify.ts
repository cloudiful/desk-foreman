import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

export function useNotify() {
  const toast = useToast()
  const { t } = useI18n()

  function success(title: string, description?: string): void {
    toast.add({ title, description, color: 'success' })
  }

  function error(title: string, description?: string): void {
    if (description === '') return
    toast.add({ title, description, color: 'error' })
  }

  async function copy(
    text: string,
    label = t('shared.notifications.copied'),
  ): Promise<void> {
    try {
      await navigator.clipboard.writeText(text)
      success(label)
    } catch {
      error(t('shared.notifications.copyFailed'))
    }
  }

  return { success, error, copy }
}
