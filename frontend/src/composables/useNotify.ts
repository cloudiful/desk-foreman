import { useToast } from '@nuxt/ui/runtime/composables/useToast'

export function useNotify() {
  const toast = useToast()

  function success(title: string, description?: string): void {
    toast.add({ title, description, color: 'success' })
  }

  function error(title: string, description?: string): void {
    toast.add({ title, description, color: 'error' })
  }

  async function copy(text: string, label = 'Copied'): Promise<void> {
    try {
      await navigator.clipboard.writeText(text)
      success(label)
    } catch {
      error('Copy failed')
    }
  }

  return { success, error, copy }
}
