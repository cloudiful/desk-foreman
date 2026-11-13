import { createContext, useContext } from 'react'
import type {
  AuthLoginRequest,
  ChangePasswordRequest,
  UserResponse,
} from '@/api/generated'

export interface SessionValue {
  user: UserResponse | null
  isLoading: boolean
  sessionEpoch: number
  isSessionCurrent: (epoch: number) => boolean
  login: (body: AuthLoginRequest) => Promise<void>
  logout: () => Promise<void>
  changePassword: (body: ChangePasswordRequest) => Promise<void>
}

export const SessionContext = createContext<SessionValue | null>(null)

export function useSession(): SessionValue {
  const context = useContext(SessionContext)
  if (!context) {
    throw new Error('useSession must be used inside AuthProvider')
  }
  return context
}
