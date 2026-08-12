import { computed, reactive, ref } from 'vue'

export function useAsyncData<T>(loader: () => Promise<T>) {
  const data = ref<T | null>(null) as { value: T | null }
  const loading = ref(false)
  const error = ref('')

  async function load(): Promise<void> {
    loading.value = true
    error.value = ''
    try {
      data.value = await loader()
    } catch (err) {
      error.value = err instanceof Error ? err.message : 'Request failed'
    } finally {
      loading.value = false
    }
  }

  return { data, loading, error, load }
}

export interface PageState {
  limit: number
  offset: number
  total: number
  sortBy: string | undefined
  sortDir: 'asc' | 'desc' | undefined
}

export function usePageState(initialLimit = 20) {
  const state = reactive<PageState>({
    limit: initialLimit,
    offset: 0,
    total: 0,
    sortBy: undefined,
    sortDir: undefined,
  })

  const page = computed(() => Math.floor(state.offset / state.limit) + 1)
  const pageCount = computed(() =>
    state.limit > 0 ? Math.max(1, Math.ceil(state.total / state.limit)) : 1,
  )

  function setPage(next: number): void {
    state.offset = (Math.max(1, next) - 1) * state.limit
  }

  function reset(): void {
    state.offset = 0
  }

  return { state, page, pageCount, setPage, reset }
}
