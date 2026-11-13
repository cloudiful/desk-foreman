import { useForm } from '@tanstack/react-form'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { CircleAlert } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  createApplicationMutation,
  listApplicationsQueryKey,
  updateApplicationMutation,
  type ApplicationResponse,
  type CreateApplicationRequest,
  type UpdateApplicationRequest,
} from '@/api/generated'
import { errorMessage } from '@/api/http'
import { firstError } from '@/lib/form-errors'
import { ScopeCheckboxGroup } from '@/components/scope-checkbox-group'
import {
  TOKEN_SCOPES,
  knownScopes,
  unknownScopes,
  type TokenScope,
} from '@/features/access/scopes'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
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
import { Switch } from '@/components/ui/switch'

function toNumberOrNull(value: string): number | null {
  if (!value.trim()) return null
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : null
}

interface ApplicationFormDialogProps {
  application: ApplicationResponse | null
  onOpenChange: (open: boolean) => void
}

export function ApplicationFormDialog({
  application,
  onOpenChange,
}: ApplicationFormDialogProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [serverError, setServerError] = useState('')
  const isEdit = application !== null
  const retainedScopes = application
    ? unknownScopes(application.default_scopes)
    : []

  const createApplication = useMutation({ ...createApplicationMutation() })
  const updateApplication = useMutation({ ...updateApplicationMutation() })

  function validateLimit(value: string): string | undefined {
    if (!value.trim()) return undefined
    const parsed = Number(value)
    if (!Number.isFinite(parsed) || parsed < 0) {
      return t('applications.validation.limitNonNegative')
    }
    return undefined
  }

  const form = useForm({
    defaultValues: {
      name: application?.name ?? '',
      workspace_template: application?.workspace_template ?? '',
      default_shell: application?.default_shell ?? '/bin/bash',
      scopes: (application
        ? knownScopes(application.default_scopes)
        : [...TOKEN_SCOPES]) as TokenScope[],
      max_timeout_ms:
        application?.max_timeout_ms !== null &&
        application?.max_timeout_ms !== undefined
          ? String(application.max_timeout_ms)
          : '',
      max_output_bytes:
        application?.max_output_bytes !== null &&
        application?.max_output_bytes !== undefined
          ? String(application.max_output_bytes)
          : '',
      max_file_bytes:
        application?.max_file_bytes !== null &&
        application?.max_file_bytes !== undefined
          ? String(application.max_file_bytes)
          : '',
      max_sessions:
        application?.max_sessions !== null &&
        application?.max_sessions !== undefined
          ? String(application.max_sessions)
          : '',
      network_enabled: application?.network_enabled ?? true,
      is_active: application?.is_active ?? true,
    },
    onSubmit: async ({ value }) => {
      setServerError('')
      const common = {
        name: value.name.trim(),
        workspace_template: value.workspace_template.trim() || null,
        default_shell: value.default_shell.trim() || null,
        default_scopes: [...value.scopes, ...retainedScopes],
        max_timeout_ms: toNumberOrNull(value.max_timeout_ms),
        max_output_bytes: toNumberOrNull(value.max_output_bytes),
        max_file_bytes: toNumberOrNull(value.max_file_bytes),
        max_sessions: toNumberOrNull(value.max_sessions),
        network_enabled: value.network_enabled,
      }
      try {
        if (isEdit && application) {
          const body: UpdateApplicationRequest = {
            ...common,
            is_active: value.is_active,
          }
          await updateApplication.mutateAsync({
            path: { application_id: application.application_id },
            body,
          })
          toast.success(t('applications.notifications.updated'), {
            description: common.name,
          })
        } else {
          const body: CreateApplicationRequest = common
          await createApplication.mutateAsync({ body })
          toast.success(t('applications.notifications.created'), {
            description: common.name,
          })
        }
        await queryClient.invalidateQueries({
          queryKey: listApplicationsQueryKey(),
        })
        onOpenChange(false)
      } catch (error) {
        setServerError(errorMessage(error, t('applications.errors.save')))
      }
    },
  })

  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!form.state.isSubmitting) onOpenChange(next)
      }}
    >
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>
            {isEdit
              ? t('applications.drawer.editTitle')
              : t('applications.drawer.createTitle')}
          </DialogTitle>
          <DialogDescription>
            {isEdit
              ? t('applications.drawer.editDescription')
              : t('applications.drawer.createDescription')}
          </DialogDescription>
        </DialogHeader>

        <form
          id="application-form"
          className="space-y-6"
          onSubmit={(event) => {
            event.preventDefault()
            event.stopPropagation()
            void form.handleSubmit()
          }}
        >
          <div className="space-y-4">
            <div className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
              {t('applications.sections.general')}
            </div>

            <FieldGroup>
              <form.Field
                name="name"
                validators={{
                  onBlur: ({ value }) =>
                    value.trim()
                      ? undefined
                      : t('applications.validation.nameRequired'),
                  onSubmit: ({ value }) =>
                    value.trim()
                      ? undefined
                      : t('applications.validation.nameRequired'),
                }}
              >
                {(field) => (
                  <Field
                    data-invalid={Boolean(firstError(field.state.meta.errors))}
                  >
                    <FieldLabel htmlFor="application-name">
                      {t('applications.fields.name')}
                    </FieldLabel>
                    <Input
                      id="application-name"
                      name="name"
                      autoComplete="off"
                      placeholder={t('applications.placeholders.name')}
                      value={field.state.value}
                      aria-invalid={
                        firstError(field.state.meta.errors) ? true : undefined
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

              <form.Field name="workspace_template">
                {(field) => (
                  <Field>
                    <FieldLabel htmlFor="application-workspace-template">
                      {t('applications.fields.workspaceTemplate')}
                    </FieldLabel>
                    <Input
                      id="application-workspace-template"
                      name="workspace_template"
                      autoComplete="off"
                      placeholder={t(
                        'applications.placeholders.workspaceTemplate',
                      )}
                      value={field.state.value}
                      onChange={(event) =>
                        field.handleChange(event.target.value)
                      }
                    />
                    <FieldDescription>
                      {t('applications.hints.workspaceTemplate')}
                    </FieldDescription>
                  </Field>
                )}
              </form.Field>

              <form.Field name="default_shell">
                {(field) => (
                  <Field>
                    <FieldLabel htmlFor="application-default-shell">
                      {t('applications.fields.defaultShell')}
                    </FieldLabel>
                    <Input
                      id="application-default-shell"
                      name="default_shell"
                      autoComplete="off"
                      placeholder={t('applications.placeholders.defaultShell')}
                      value={field.state.value}
                      onChange={(event) =>
                        field.handleChange(event.target.value)
                      }
                    />
                  </Field>
                )}
              </form.Field>

              <form.Field name="scopes">
                {(field) => (
                  <Field>
                    <FieldLabel>{t('shared.token.scopes')}</FieldLabel>
                    <ScopeCheckboxGroup
                      idPrefix="application-scope"
                      value={field.state.value}
                      onChange={field.handleChange}
                    />
                    {retainedScopes.length ? (
                      <div className="space-y-1">
                        <FieldDescription>
                          {t('applications.retainedScopes', {
                            scopes: retainedScopes.join(', '),
                          })}
                        </FieldDescription>
                        <div className="flex flex-wrap gap-1">
                          {retainedScopes.map((scope) => (
                            <Badge key={scope} variant="secondary">
                              {scope}
                            </Badge>
                          ))}
                        </div>
                      </div>
                    ) : null}
                  </Field>
                )}
              </form.Field>
            </FieldGroup>
          </div>

          <div className="space-y-4">
            <div className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
              {t('applications.sections.resourceLimits')}
            </div>
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <form.Field
                name="max_timeout_ms"
                validators={{ onSubmit: ({ value }) => validateLimit(value) }}
              >
                {(field) => (
                  <Field
                    data-invalid={Boolean(firstError(field.state.meta.errors))}
                  >
                    <FieldLabel htmlFor="application-max-timeout">
                      {t('applications.fields.maxTimeout')}
                    </FieldLabel>
                    <Input
                      id="application-max-timeout"
                      name="max_timeout_ms"
                      type="number"
                      min="0"
                      placeholder={t('applications.placeholders.unlimited')}
                      value={field.state.value}
                      aria-invalid={
                        firstError(field.state.meta.errors) ? true : undefined
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

              <form.Field
                name="max_output_bytes"
                validators={{ onSubmit: ({ value }) => validateLimit(value) }}
              >
                {(field) => (
                  <Field
                    data-invalid={Boolean(firstError(field.state.meta.errors))}
                  >
                    <FieldLabel htmlFor="application-max-output">
                      {t('applications.fields.maxOutput')}
                    </FieldLabel>
                    <Input
                      id="application-max-output"
                      name="max_output_bytes"
                      type="number"
                      min="0"
                      placeholder={t('applications.placeholders.unlimited')}
                      value={field.state.value}
                      aria-invalid={
                        firstError(field.state.meta.errors) ? true : undefined
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

              <form.Field
                name="max_file_bytes"
                validators={{ onSubmit: ({ value }) => validateLimit(value) }}
              >
                {(field) => (
                  <Field
                    data-invalid={Boolean(firstError(field.state.meta.errors))}
                  >
                    <FieldLabel htmlFor="application-max-file">
                      {t('applications.fields.maxFile')}
                    </FieldLabel>
                    <Input
                      id="application-max-file"
                      name="max_file_bytes"
                      type="number"
                      min="0"
                      placeholder={t('applications.placeholders.unlimited')}
                      value={field.state.value}
                      aria-invalid={
                        firstError(field.state.meta.errors) ? true : undefined
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

              <form.Field
                name="max_sessions"
                validators={{ onSubmit: ({ value }) => validateLimit(value) }}
              >
                {(field) => (
                  <Field
                    data-invalid={Boolean(firstError(field.state.meta.errors))}
                  >
                    <FieldLabel htmlFor="application-max-sessions">
                      {t('applications.fields.maxSessions')}
                    </FieldLabel>
                    <Input
                      id="application-max-sessions"
                      name="max_sessions"
                      type="number"
                      min="0"
                      placeholder={t('applications.placeholders.unlimited')}
                      value={field.state.value}
                      aria-invalid={
                        firstError(field.state.meta.errors) ? true : undefined
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
            </div>

            <div className="space-y-3 rounded-lg border p-4">
              <form.Field name="network_enabled">
                {(field) => (
                  <div className="flex items-center justify-between gap-4">
                    <div>
                      <div className="text-sm font-medium">
                        {t('applications.fields.networkAccess')}
                      </div>
                      <div className="text-xs text-muted-foreground">
                        {t('applications.hints.networkAccess')}
                      </div>
                    </div>
                    <Switch
                      checked={field.state.value}
                      aria-label={t('applications.fields.networkAccess')}
                      onCheckedChange={field.handleChange}
                    />
                  </div>
                )}
              </form.Field>
              {isEdit ? (
                <form.Field name="is_active">
                  {(field) => (
                    <div className="flex items-center justify-between gap-4">
                      <div>
                        <div className="text-sm font-medium">
                          {t('applications.fields.active')}
                        </div>
                        <div className="text-xs text-muted-foreground">
                          {t('applications.hints.active')}
                        </div>
                      </div>
                      <Switch
                        checked={field.state.value}
                        aria-label={t('applications.fields.active')}
                        onCheckedChange={field.handleChange}
                      />
                    </div>
                  )}
                </form.Field>
              ) : null}
            </div>
          </div>

          {serverError ? (
            <Alert variant="destructive">
              <CircleAlert aria-hidden="true" />
              <AlertDescription>{serverError}</AlertDescription>
            </Alert>
          ) : null}
        </form>

        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            disabled={form.state.isSubmitting}
            onClick={() => onOpenChange(false)}
          >
            {t('shared.confirm.cancel')}
          </Button>
          <form.Subscribe
            selector={(state) => [state.canSubmit, state.isSubmitting] as const}
          >
            {([canSubmit, isSubmitting]) => (
              <Button
                type="submit"
                form="application-form"
                disabled={!canSubmit || isSubmitting}
              >
                {isEdit
                  ? t('applications.actions.saveChanges')
                  : t('applications.actions.create')}
              </Button>
            )}
          </form.Subscribe>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
