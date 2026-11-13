import { useQuery, useQueryClient, type Query } from '@tanstack/react-query'
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { meQueryKey } from '@/api/generated'
import type {
  AuthLoginRequest,
  ChangePasswordRequest,
  UserResponse,
} from '@/api/generated'
import {
  setSessionEstablishedSource,
  setSessionGenerationSource,
  setUnauthorizedHandler,
} from '@/api/http'
import { SessionContext, type SessionValue } from './session-context'
import {
  changeCurrentPassword,
  fetchCurrentUser,
  loginWithPassword,
  logoutCurrentUser,
} from '@/api/session'

function isSessionQuery(query: Query): boolean {
  const key = query.queryKey[0]
  return (
    typeof key === 'object' &&
    key !== null &&
    '_id' in key &&
    (key as { _id?: string })._id === 'me'
  )
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient()
  const epochRef = useRef(0)
  const transitionRef = useRef(0)
  const previousIdentityRef = useRef<number | null | undefined>(undefined)
  const [sessionEpoch, setSessionEpoch] = useState(0)

  const userQuery = useQuery({
    queryKey: meQueryKey(),
    queryFn: async ({ signal }) => {
      const epoch = epochRef.current
      const user = await fetchCurrentUser(signal)
      if (epochRef.current !== epoch) {
        return (
          queryClient.getQueryData<UserResponse | null>(meQueryKey()) ?? null
        )
      }
      return user
    },
    retry: false,
    staleTime: 30_000,
  })

  const advanceSessionEpoch = useCallback(() => {
    epochRef.current += 1
    setSessionEpoch(epochRef.current)
  }, [])

  const isSessionCurrent = useCallback(
    (epoch: number) => epochRef.current === epoch,
    [],
  )

  const beginIdentityTransition = useCallback(() => {
    transitionRef.current += 1
    return transitionRef.current
  }, [])

  const clearOtherSessionCaches = useCallback(() => {
    advanceSessionEpoch()
    void queryClient.cancelQueries()
    queryClient.removeQueries({ predicate: (query) => !isSessionQuery(query) })
    queryClient.getMutationCache().clear()
  }, [advanceSessionEpoch, queryClient])

  const clearSessionCaches = useCallback(() => {
    clearOtherSessionCaches()
    queryClient.setQueryData(meQueryKey(), null)
  }, [clearOtherSessionCaches, queryClient])

  useEffect(() => {
    setSessionGenerationSource(() => epochRef.current)
    setSessionEstablishedSource(
      () => queryClient.getQueryData(meQueryKey()) != null,
    )
    setUnauthorizedHandler(clearSessionCaches)
    return () => {
      setSessionGenerationSource(() => 0)
      setSessionEstablishedSource(() => false)
      setUnauthorizedHandler(() => {})
    }
  }, [clearSessionCaches, queryClient])

  useEffect(() => {
    const identity = userQuery.data?.user_id ?? null
    const previous = previousIdentityRef.current
    previousIdentityRef.current = identity
    if (previous != null && identity != null && previous !== identity) {
      clearOtherSessionCaches()
    }
  }, [clearOtherSessionCaches, userQuery.data])

  const login = useCallback(
    async (body: AuthLoginRequest) => {
      const transition = beginIdentityTransition()
      const result = await loginWithPassword(body)
      if (transitionRef.current !== transition) return
      clearSessionCaches()
      queryClient.setQueryData(meQueryKey(), result.user)
    },
    [beginIdentityTransition, clearSessionCaches, queryClient],
  )

  const logout = useCallback(async () => {
    const transition = beginIdentityTransition()
    try {
      await logoutCurrentUser()
    } finally {
      if (transitionRef.current === transition) clearSessionCaches()
    }
  }, [beginIdentityTransition, clearSessionCaches])

  const changePassword = useCallback(
    async (body: ChangePasswordRequest) => {
      const epoch = epochRef.current
      await changeCurrentPassword(body)
      if (epochRef.current !== epoch) return
      await queryClient.invalidateQueries({ queryKey: meQueryKey() })
    },
    [queryClient],
  )

  const value = useMemo<SessionValue>(
    () => ({
      user: userQuery.data ?? null,
      isLoading: userQuery.isPending,
      sessionEpoch,
      isSessionCurrent,
      login,
      logout,
      changePassword,
    }),
    [
      userQuery.data,
      userQuery.isPending,
      sessionEpoch,
      isSessionCurrent,
      login,
      logout,
      changePassword,
    ],
  )

  return (
    <SessionContext.Provider value={value}>{children}</SessionContext.Provider>
  )
}
