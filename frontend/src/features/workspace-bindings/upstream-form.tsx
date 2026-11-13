import { useForm } from '@tanstack/react-form'
import { useQueryClient } from '@tanstack/react-query'
import { CircleAlert } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  createWorkspaceGitUpstreamMutation,
  listWorkspaceGitUpstreamsQueryKey,
  updateWorkspaceGitUpstreamMutation,
  type CreateWorkspaceGitUpstreamRequest,
  type UpdateWorkspaceGitUpstreamRequest,
  type WorkspaceGitUpstreamResponse,
} from '@/api/generated'
import { errorMessage } from '@/api/http'
import { firstError } from '@/lib/form-errors'
import {
  SENSITIVE_MUTATION_KEYS,
  useSensitiveMutation,
} from '@/lib/sensitive-mutation'
import { useSession } from '@/features/auth/session-context'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
} from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'

interface UpstreamFormProps {
  bindingId: number
  upstream: WorkspaceGitUpstreamResponse | null
  onCancel: () => void
  onSaved: () => void
  onSubmittingChange: (submitting: boolean) => void
}

export function UpstreamForm({
  bindingId,
  upstream,
  onCancel,
  onSaved,
  onSubmittingChange,
}: UpstreamFormProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const { sessionEpoch, isSessionCurrent } = useSession()
  const [serverError, setServerError] = useState('')
  const mountedRef = useRef(true)
  const submittingRef = useRef(false)
  const isEdit = upstream !== null

  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
      onSubmittingChange(false)
    }
  }, [onSubmittingChange])

  const { mutation: createUpstream, discard: discardCreateUpstream } =
    useSensitiveMutation({
      ...createWorkspaceGitUpstreamMutation(),
      mutationKey: SENSITIVE_MUTATION_KEYS.createWorkspaceGitUpstream,
    })
  const { mutation: updateUpstream, discard: discardUpdateUpstream } =
    useSensitiveMutation({
      ...updateWorkspaceGitUpstreamMutation(),
      mutationKey: SENSITIVE_MUTATION_KEYS.updateWorkspaceGitUpstream,
    })

  function credentialError(
    username: string,
    secret: string,
    clear: boolean,
  ): string | undefined {
    if (clear) {
      if (username || secret) {
        return t('workspaceBindings.upstreams.errors.clearConflict')
      }
      return undefined
    }
    if (Boolean(username) !== Boolean(secret)) {
      return t('workspaceBindings.upstreams.errors.pairRequired')
    }
    return undefined
  }

  const form = useForm({
    defaultValues: {
      remote_alias: upstream?.remote_alias ?? '',
      target_url: upstream?.target_url ?? '',
      credential_username: '',
      credential_secret: '',
      clear_credential: false,
    },
    onSubmit: async ({ value }) => {
      setServerError('')
      const epoch = sessionEpoch
      submittingRef.current = true
      onSubmittingChange(true)
      const hasCredential =
        value.credential_username.length > 0 &&
        value.credential_secret.length > 0
      try {
        if (isEdit && upstream) {
          const body: UpdateWorkspaceGitUpstreamRequest = {
            remote_alias: value.remote_alias.trim(),
            target_url: value.target_url.trim(),
            credential_username: hasCredential
              ? value.credential_username
              : undefined,
            credential_secret: hasCredential
              ? value.credential_secret
              : undefined,
            clear_credential: value.clear_credential || undefined,
          }
          await updateUpstream.mutateAsync({
            path: { binding_id: bindingId, upstream_id: upstream.upstream_id },
            body,
          })
          if (!isSessionCurrent(epoch) || !mountedRef.current) return
          form.reset()
          discardUpdateUpstream()
          toast.success(t('workspaceBindings.upstreams.notifications.updated'))
        } else {
          const body: CreateWorkspaceGitUpstreamRequest = {
            remote_alias: value.remote_alias.trim(),
            target_url: value.target_url.trim(),
            credential_username: hasCredential
              ? value.credential_username
              : undefined,
            credential_secret: hasCredential
              ? value.credential_secret
              : undefined,
          }
          await createUpstream.mutateAsync({
            path: { binding_id: bindingId },
            body,
          })
          if (!isSessionCurrent(epoch) || !mountedRef.current) return
          form.reset()
          discardCreateUpstream()
          toast.success(t('workspaceBindings.upstreams.notifications.created'))
        }
        await queryClient.invalidateQueries({
          queryKey: listWorkspaceGitUpstreamsQueryKey({
            path: { binding_id: bindingId },
          }),
        })
        if (!isSessionCurrent(epoch) || !mountedRef.current) return
        onSaved()
      } catch (error) {
        if (!isSessionCurrent(epoch) || !mountedRef.current) return
        setServerError(
          errorMessage(error, t('workspaceBindings.errors.saveUpstream')),
        )
        if (isEdit) {
          discardUpdateUpstream()
        } else {
          discardCreateUpstream()
        }
      } finally {
        submittingRef.current = false
        onSubmittingChange(false)
      }
    },
  })

  function close(): void {
    if (submittingRef.current) return
    form.reset()
    discardCreateUpstream()
    discardUpdateUpstream()
    onCancel()
  }

  return (
    <form
      id="workspace-git-upstream-form"
      className="space-y-4 rounded-xl border p-4"
      onSubmit={(event) => {
        event.preventDefault()
        event.stopPropagation()
        void form.handleSubmit()
      }}
    >
      <h3 className="text-sm font-semibold">
        {isEdit
          ? t('workspaceBindings.upstreams.form.editTitle')
          : t('workspaceBindings.upstreams.form.createTitle')}
      </h3>

      <FieldGroup>
        <form.Field
          name="remote_alias"
          validators={{
            onBlur: ({ value }) =>
              value.trim()
                ? undefined
                : t('workspaceBindings.upstreams.validation.aliasRequired'),
            onSubmit: ({ value }) =>
              value.trim()
                ? undefined
                : t('workspaceBindings.upstreams.validation.aliasRequired'),
          }}
        >
          {(field) => (
            <Field data-invalid={Boolean(firstError(field.state.meta.errors))}>
              <FieldLabel htmlFor="upstream-alias">
                {t('workspaceBindings.upstreams.fields.alias')}
              </FieldLabel>
              <Input
                id="upstream-alias"
                name="remote_alias"
                autoComplete="off"
                placeholder={t('workspaceBindings.upstreams.fields.aliasHint')}
                value={field.state.value}
                aria-invalid={
                  firstError(field.state.meta.errors) ? true : undefined
                }
                onChange={(event) => field.handleChange(event.target.value)}
                onBlur={field.handleBlur}
              />
              <FieldError>{firstError(field.state.meta.errors)}</FieldError>
            </Field>
          )}
        </form.Field>

        <form.Field
          name="target_url"
          validators={{
            onBlur: ({ value }) =>
              value.trim()
                ? undefined
                : t('workspaceBindings.upstreams.validation.targetUrlRequired'),
            onSubmit: ({ value }) =>
              value.trim()
                ? undefined
                : t('workspaceBindings.upstreams.validation.targetUrlRequired'),
          }}
        >
          {(field) => (
            <Field data-invalid={Boolean(firstError(field.state.meta.errors))}>
              <FieldLabel htmlFor="upstream-target-url">
                {t('workspaceBindings.upstreams.fields.targetUrl')}
              </FieldLabel>
              <Input
                id="upstream-target-url"
                name="target_url"
                autoComplete="off"
                className="font-mono"
                placeholder={t(
                  'workspaceBindings.upstreams.fields.targetUrlHint',
                )}
                value={field.state.value}
                aria-invalid={
                  firstError(field.state.meta.errors) ? true : undefined
                }
                onChange={(event) => field.handleChange(event.target.value)}
                onBlur={field.handleBlur}
              />
              <FieldError>{firstError(field.state.meta.errors)}</FieldError>
            </Field>
          )}
        </form.Field>

        <form.Field
          name="credential_username"
          validators={{
            onSubmit: ({ value, fieldApi }) =>
              credentialError(
                value,
                fieldApi.form.getFieldValue('credential_secret'),
                fieldApi.form.getFieldValue('clear_credential'),
              ),
          }}
        >
          {(field) => (
            <Field data-invalid={Boolean(firstError(field.state.meta.errors))}>
              <FieldLabel htmlFor="upstream-credential-username">
                {t('workspaceBindings.upstreams.fields.username')}
              </FieldLabel>
              <Input
                id="upstream-credential-username"
                name="credential_username"
                autoComplete="off"
                disabled={form.state.values.clear_credential}
                value={field.state.value}
                aria-invalid={
                  firstError(field.state.meta.errors) ? true : undefined
                }
                onChange={(event) => field.handleChange(event.target.value)}
                onBlur={field.handleBlur}
              />
              <FieldDescription>
                {t('workspaceBindings.upstreams.fields.credentialHint')}
              </FieldDescription>
              <FieldError>{firstError(field.state.meta.errors)}</FieldError>
            </Field>
          )}
        </form.Field>

        <form.Field
          name="credential_secret"
          validators={{
            onSubmit: ({ value, fieldApi }) =>
              credentialError(
                fieldApi.form.getFieldValue('credential_username'),
                value,
                fieldApi.form.getFieldValue('clear_credential'),
              ),
          }}
        >
          {(field) => (
            <Field data-invalid={Boolean(firstError(field.state.meta.errors))}>
              <FieldLabel htmlFor="upstream-credential-secret">
                {t('workspaceBindings.upstreams.fields.secret')}
              </FieldLabel>
              <Input
                id="upstream-credential-secret"
                name="credential_secret"
                type="password"
                autoComplete="new-password"
                disabled={form.state.values.clear_credential}
                placeholder={t('workspaceBindings.upstreams.fields.secretHint')}
                value={field.state.value}
                aria-invalid={
                  firstError(field.state.meta.errors) ? true : undefined
                }
                onChange={(event) => field.handleChange(event.target.value)}
                onBlur={field.handleBlur}
              />
              <FieldError>{firstError(field.state.meta.errors)}</FieldError>
            </Field>
          )}
        </form.Field>

        {isEdit ? (
          <form.Field name="clear_credential">
            {(field) => (
              <div className="flex items-center gap-2">
                <Checkbox
                  id="upstream-clear-credential"
                  checked={field.state.value}
                  onCheckedChange={(checked) => {
                    const next = checked === true
                    field.handleChange(next)
                    if (next) {
                      form.setFieldValue('credential_username', '')
                      form.setFieldValue('credential_secret', '')
                    }
                  }}
                />
                <Label htmlFor="upstream-clear-credential">
                  {t('workspaceBindings.upstreams.fields.clearCredential')}
                </Label>
              </div>
            )}
          </form.Field>
        ) : null}
      </FieldGroup>

      {serverError ? (
        <Alert variant="destructive">
          <CircleAlert aria-hidden="true" />
          <AlertDescription>{serverError}</AlertDescription>
        </Alert>
      ) : null}

      <div className="flex justify-end gap-2">
        <Button
          type="button"
          variant="outline"
          disabled={form.state.isSubmitting}
          onClick={close}
        >
          {t('workspaceBindings.upstreams.actions.cancel')}
        </Button>
        <form.Subscribe
          selector={(state) => [state.canSubmit, state.isSubmitting] as const}
        >
          {([canSubmit, isSubmitting]) => (
            <Button
              type="submit"
              form="workspace-git-upstream-form"
              disabled={!canSubmit || isSubmitting}
            >
              {isEdit
                ? t('workspaceBindings.upstreams.actions.save')
                : t('workspaceBindings.upstreams.actions.create')}
            </Button>
          )}
        </form.Subscribe>
      </div>
    </form>
  )
}
