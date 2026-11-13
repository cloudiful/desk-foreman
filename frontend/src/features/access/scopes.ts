export const TOKEN_SCOPES = [
  'workspace.read',
  'workspace.search',
  'workspace.shell',
  'workspace.patch',
] as const

export type TokenScope = (typeof TOKEN_SCOPES)[number]

export function knownScopes(scopes: string[]): TokenScope[] {
  return scopes.filter((scope): scope is TokenScope =>
    TOKEN_SCOPES.includes(scope as TokenScope),
  )
}

export function unknownScopes(scopes: string[]): string[] {
  return scopes.filter((scope) => !TOKEN_SCOPES.includes(scope as TokenScope))
}
