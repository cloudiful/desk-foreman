export function firstError(errors: readonly unknown[]): string | undefined {
  const error = errors[0]
  return typeof error === 'string' && error ? error : undefined
}
