import { client } from './generated'

export class ApiError extends Error {
  readonly status: number

  constructor(message: string, status: number) {
    super(message)
    this.name = 'ApiError'
    this.status = status
  }
}

const LOGIN_ENDPOINT = '/api/auth/login'
const ME_ENDPOINT = '/api/auth/me'
const CHANGE_PASSWORD_ENDPOINT = '/api/auth/change-password'
const INVALID_CURRENT_PASSWORD = 'invalid current password'

let unauthorizedHandler: (() => void) | undefined
let sessionGeneration: (() => number) | undefined
let sessionEstablished: (() => boolean) | undefined
const requestGenerations = new WeakMap<Request, number>()
const requestSessions = new WeakMap<Request, boolean>()
let interceptorInstalled = false

export function setUnauthorizedHandler(handler: () => void): void {
  unauthorizedHandler = handler
}

export function setSessionGenerationSource(source: () => number): void {
  sessionGeneration = source
}

export function setSessionEstablishedSource(source: () => boolean): void {
  sessionEstablished = source
}

function isLoginEndpoint(url: string): boolean {
  return url.includes(LOGIN_ENDPOINT)
}

function isMeEndpoint(url: string): boolean {
  return url.includes(ME_ENDPOINT)
}

function isChangePasswordEndpoint(url: string): boolean {
  return url.includes(CHANGE_PASSWORD_ENDPOINT)
}

async function errorReason(response: Response): Promise<string | undefined> {
  try {
    const body = (await response.clone().json()) as { error?: unknown }
    return typeof body.error === 'string' ? body.error : undefined
  } catch {
    return undefined
  }
}

function belongsToCurrentSession(request: Request): boolean {
  if (!sessionGeneration) return true
  const dispatchedAt = requestGenerations.get(request)
  return dispatchedAt === undefined || dispatchedAt === sessionGeneration()
}

export function configureHttpClient(): void {
  client.setConfig({
    baseUrl: window.location.origin,
    credentials: 'include',
  })

  if (interceptorInstalled) return
  interceptorInstalled = true

  client.interceptors.request.use((request) => {
    requestGenerations.set(request, sessionGeneration?.() ?? 0)
    requestSessions.set(request, sessionEstablished?.() ?? false)
    return request
  })

  client.interceptors.response.use(async (response, request) => {
    if (response.status !== 401) return response
    if (isLoginEndpoint(response.url)) return response
    if (isMeEndpoint(response.url) && !requestSessions.get(request)) {
      return response
    }
    if (
      isChangePasswordEndpoint(response.url) &&
      (await errorReason(response)) === INVALID_CURRENT_PASSWORD
    ) {
      return response
    }
    if (belongsToCurrentSession(request)) unauthorizedHandler?.()
    return response
  })
}

export async function responseErrorMessage(
  response: Response | undefined,
  fallback: string,
): Promise<string> {
  if (!response) return fallback
  const reason = await errorReason(response)
  if (reason?.trim()) return reason
  return `${fallback} (${response.status})`
}

export function errorMessage(error: unknown, fallback: string): string {
  if (typeof error === 'object' && error !== null && 'error' in error) {
    const value = (error as { error?: unknown }).error
    if (typeof value === 'string' && value.trim()) return value
  }
  if (error instanceof Error && error.message) return error.message
  return fallback
}
