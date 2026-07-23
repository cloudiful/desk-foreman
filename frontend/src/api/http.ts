export async function responseErrorMessage(
  response: Response | undefined,
  fallback: string,
): Promise<string> {
  if (!response) return fallback
  try {
    const body = (await response.clone().json()) as { error?: string }
    if (body?.error?.trim()) return body.error
  } catch {
    // ignore body parsing
  }
  return `${fallback} (${response.status})`
}

export async function requireOk(
  response: Response | undefined,
  fallback: string,
): Promise<void> {
  if (!response?.ok)
    throw new Error(await responseErrorMessage(response, fallback))
}
