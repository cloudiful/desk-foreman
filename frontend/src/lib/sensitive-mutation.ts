import {
  useMutation,
  useQueryClient,
  type MutationKey,
  type UseMutationOptions,
} from '@tanstack/react-query'
import { useCallback, useEffect, useRef } from 'react'

export const SENSITIVE_MUTATION_KEYS = {
  resetUserPassword: ['sensitive', 'reset-user-password'],
  createUser: ['sensitive', 'create-user'],
  createMcpToken: ['sensitive', 'create-mcp-token'],
  createApplicationToken: ['sensitive', 'create-application-token'],
  createRunnerManager: ['sensitive', 'create-runner-manager'],
  createWorkspaceGitUpstream: ['sensitive', 'create-workspace-git-upstream'],
  updateWorkspaceGitUpstream: ['sensitive', 'update-workspace-git-upstream'],
} as const satisfies Record<string, MutationKey>

interface SensitiveMutationOptions<
  TData,
  TError,
  TVariables,
  TContext,
> extends UseMutationOptions<TData, TError, TVariables, TContext> {
  mutationKey: MutationKey
}

export function useSensitiveMutation<TData, TError, TVariables, TContext>(
  options: SensitiveMutationOptions<TData, TError, TVariables, TContext>,
) {
  const queryClient = useQueryClient()
  const mutation = useMutation({ ...options, gcTime: 0 })
  const resetRef = useRef(mutation.reset)
  resetRef.current = mutation.reset

  const discard = useCallback(() => {
    resetRef.current()
    const cache = queryClient.getMutationCache()
    for (const entry of cache.findAll({
      mutationKey: options.mutationKey,
      exact: true,
    })) {
      cache.remove(entry)
    }
  }, [options.mutationKey, queryClient])

  useEffect(() => discard, [discard])

  return { mutation, discard }
}
