import { useToast } from '@nuxt/ui/composables'

export function useNotify() {
  const toast = useToast()

  function success(title: string, description?: string): void {
    toast.add({ title, description, color: 'success' })
  }

  function error(title: string, description?: string): void {
    if (description === '') return
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
