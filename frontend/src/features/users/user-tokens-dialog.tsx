import { useForm } from '@tanstack/react-form'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  createMcpTokenMutation,
  deleteMcpTokenMutation,
  listMcpTokensOptions,
  listMcpTokensQueryKey,
  type CreateMcpTokenResponse,
  type McpTokenResponse,
  type UserResponse,
} from '@/api/generated'
import { errorMessage } from '@/api/http'
import {
  SENSITIVE_MUTATION_KEYS,
  useSensitiveMutation,
} from '@/lib/sensitive-mutation'
import { useSession } from '@/features/auth/session-context'
import { ConfirmDialog } from '@/components/confirm-dialog'
import { ErrorAlert } from '@/components/error-alert'
import { TokenReveal } from '@/components/token-reveal'
import { ScopeCheckboxGroup } from '@/components/scope-checkbox-group'
import { TablePagination } from '@/components/table-pagination'
import { TokenCards } from '@/features/access/token-cards'
import { TOKEN_SCOPES, type TokenScope } from '@/features/access/scopes'
import { firstError } from '@/lib/form-errors'
import { PAGE_SIZE, pageCount, pageOffset } from '@/utils/pagination'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
} from '@/components/ui/field'
import { Input } from '@/components/ui/input'

interface UserTokensDialogProps {
  user: UserResponse
  onOpenChange: (open: boolean) => void
}

export function UserTokensDialog({
  user,
  onOpenChange,
}: UserTokensDialogProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const { sessionEpoch, isSessionCurrent } = useSession()
  const mountedRef = useRef(true)
  const submittingRef = useRef(false)
  const [page, setPage] = useState(1)
  const [revealedToken, setRevealedToken] =
    useState<CreateMcpTokenResponse | null>(null)
  const [revokeTarget, setRevokeTarget] = useState<McpTokenResponse | null>(
    null,
  )

  const tokensQuery = useQuery(
    listMcpTokensOptions({
      query: {
        user_id: user.user_id,
        limit: PAGE_SIZE,
        offset: pageOffset(page, PAGE_SIZE),
      },
    }),
  )

  const { mutation: createToken, discard: discardCreateToken } =
    useSensitiveMutation({
      ...createMcpTokenMutation(),
      mutationKey: SENSITIVE_MUTATION_KEYS.createMcpToken,
    })
  const deleteToken = useMutation({ ...deleteMcpTokenMutation() })

  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
    }
  }, [])

  const form = useForm({
    defaultValues: {
      token_name: '',
      scopes: [...TOKEN_SCOPES] as TokenScope[],
      expires_at: '',
    },
    onSubmit: async ({ value }) => {
      const epoch = sessionEpoch
      submittingRef.current = true
      try {
        const result = await createToken.mutateAsync({
          body: {
            user_id: user.user_id,
            token_name: value.token_name.trim(),
            scopes: value.scopes,
            expires_at: value.expires_at
              ? new Date(value.expires_at).toISOString()
              : null,
          },
        })
        if (!isSessionCurrent(epoch) || !mountedRef.current) return
        setRevealedToken(result)
        form.reset()
        discardCreateToken()
        toast.success(t('shared.token.createdNotification'))
        await queryClient.invalidateQueries({
          queryKey: listMcpTokensQueryKey({
            query: { user_id: user.user_id },
          }),
        })
      } catch (error) {
        if (!isSessionCurrent(epoch) || !mountedRef.current) return
        toast.error(t('users.errors.createToken'), {
          description: errorMessage(error, t('users.errors.createToken')),
        })
        discardCreateToken()
      } finally {
        submittingRef.current = false
      }
    },
  })

  function clearRevealedToken(): void {
    setRevealedToken(null)
    discardCreateToken()
  }

  function close(): void {
    if (submittingRef.current) return
    clearRevealedToken()
    setRevokeTarget(null)
    form.reset()
    onOpenChange(false)
  }

  async function confirmRevoke(): Promise<void> {
    if (!revokeTarget) return
    try {
      await deleteToken.mutateAsync({
        path: { token_id: revokeTarget.token_id },
      })
      toast.success(t('shared.token.revokedNotification'), {
        description: revokeTarget.token_name,
      })
      setRevokeTarget(null)
      await queryClient.invalidateQueries({
        queryKey: listMcpTokensQueryKey({
          query: { user_id: user.user_id },
        }),
      })
    } catch (error) {
      toast.error(t('users.errors.revokeToken'), {
        description: errorMessage(error, t('users.errors.revokeToken')),
      })
    }
  }

  const rows = tokensQuery.data?.items ?? []
  const total = tokensQuery.data?.total ?? 0

  return (
    <>
      <Dialog
        open
        onOpenChange={(next) => {
          if (!next) close()
        }}
      >
        <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>{t('users.tokens.title')}</DialogTitle>
            <DialogDescription>
              {user.display_name} · @{user.login_name}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-6">
            <div className="space-y-4 rounded-lg border p-4">
              <div className="text-sm font-semibold">
                {t('shared.token.createTitle')}
              </div>
              <form
                id="user-token-form"
                className="space-y-4"
                onSubmit={(event) => {
                  event.preventDefault()
                  event.stopPropagation()
                  submittingRef.current = true
                  void form.handleSubmit().finally(() => {
                    submittingRef.current = false
                  })
                }}
              >
                <FieldGroup>
                  <form.Field
                    name="token_name"
                    validators={{
                      onSubmit: ({ value }) =>
                        value.trim()
                          ? undefined
                          : t('users.validation.tokenNameRequired'),
                    }}
                  >
                    {(field) => (
                      <Field
                        data-invalid={Boolean(
                          firstError(field.state.meta.errors),
                        )}
                      >
                        <FieldLabel htmlFor="user-token-name">
                          {t('shared.token.tokenName')}
                        </FieldLabel>
                        <Input
                          id="user-token-name"
                          name="token_name"
                          autoComplete="off"
                          placeholder={t('users.placeholders.tokenName')}
                          value={field.state.value}
                          aria-invalid={
                            firstError(field.state.meta.errors)
                              ? true
                              : undefined
                          }
                          onChange={(event) =>
                            field.handleChange(event.target.value)
                          }
                          onBlur={field.handleBlur}
                        />
                        <FieldError>
                          {firstError(field.state.meta.errors)}
                        </FieldError>
                      </Field>
                    )}
                  </form.Field>

                  <form.Field name="scopes">
                    {(field) => (
                      <Field>
                        <FieldLabel>{t('shared.token.scopes')}</FieldLabel>
                        <ScopeCheckboxGroup
                          idPrefix="user-token-scope"
                          value={field.state.value}
                          onChange={field.handleChange}
                        />
                      </Field>
                    )}
                  </form.Field>

                  <form.Field name="expires_at">
                    {(field) => (
                      <Field>
                        <FieldLabel htmlFor="user-token-expires">
                          {t('shared.token.expiresAt')}
                        </FieldLabel>
                        <Input
                          id="user-token-expires"
                          name="expires_at"
                          type="datetime-local"
                          value={field.state.value}
                          onChange={(event) =>
                            field.handleChange(event.target.value)
                          }
                        />
                        <FieldDescription>
                          {t('shared.token.optional')}
                        </FieldDescription>
                      </Field>
                    )}
                  </form.Field>
                </FieldGroup>

                <form.Subscribe
                  selector={(state) =>
                    [state.canSubmit, state.isSubmitting] as const
                  }
                >
                  {([canSubmit, isSubmitting]) => (
                    <Button
                      type="submit"
                      form="user-token-form"
                      className="w-full"
                      disabled={!canSubmit || isSubmitting}
                    >
                      <Plus aria-hidden="true" />
                      {t('shared.token.create')}
                    </Button>
                  )}
                </form.Subscribe>
              </form>
            </div>

            {revealedToken ? (
              <TokenReveal
                token={revealedToken.token}
                onDismiss={clearRevealedToken}
              />
            ) : null}

            {tokensQuery.error ? (
              <ErrorAlert
                error={errorMessage(
                  tokensQuery.error,
                  t('users.errors.loadTokens'),
                )}
                onRetry={() => void tokensQuery.refetch()}
              />
            ) : null}

            <div>
              <div className="mb-2 text-sm font-semibold">
                {t('shared.token.active')}
              </div>
              <TokenCards
                tokens={rows}
                emptyLabel={t('users.tokens.noTokens')}
                onRevoke={setRevokeTarget}
              />
              <TablePagination
                page={page}
                totalPages={pageCount(total, PAGE_SIZE)}
                onPageChange={setPage}
              />
            </div>
          </div>

          <DialogFooter>
            <form.Subscribe selector={(state) => state.isSubmitting}>
              {(isSubmitting) => (
                <Button
                  type="button"
                  variant="outline"
                  disabled={isSubmitting}
                  onClick={close}
                >
                  {t('shared.confirm.cancel')}
                </Button>
              )}
            </form.Subscribe>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        open={revokeTarget !== null}
        title={t('users.confirmations.revokeTitle')}
        description={
          revokeTarget
            ? t('users.confirmations.revokeDescription', {
                tokenName: revokeTarget.token_name,
              })
            : undefined
        }
        confirmLabel={t('shared.token.revoke')}
        cancelLabel={t('shared.confirm.cancel')}
        destructive
        pending={deleteToken.isPending}
        onConfirm={() => void confirmRevoke()}
        onOpenChange={(open) => {
          if (!open && !deleteToken.isPending) setRevokeTarget(null)
        }}
      />
    </>
  )
}
