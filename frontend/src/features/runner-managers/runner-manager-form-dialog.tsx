import { useForm } from '@tanstack/react-form'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { CircleAlert } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  createRunnerManagerMutation,
  listRunnerManagersQueryKey,
  updateRunnerManagerMutation,
  type CreateRunnerManagerRequest,
  type RunnerManagerResponse,
  type UpdateRunnerManagerRequest,
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

const DEFAULT_ENDPOINT = 'http://desk-foreman-runner-manager:3001'

function toNumber(value: string, fallback: number): number {
  if (!value.trim()) return fallback
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : fallback
}

interface RunnerManagerFormDialogProps {
  manager: RunnerManagerResponse | null
  onCreatedToken: (token: string) => void
  onOpenChange: (open: boolean) => void
}

export function RunnerManagerFormDialog({
  manager,
  onCreatedToken,
  onOpenChange,
}: RunnerManagerFormDialogProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const { sessionEpoch, isSessionCurrent } = useSession()
  const mountedRef = useRef(true)
  const submittingRef = useRef(false)
  const [serverError, setServerError] = useState('')
  const isEdit = manager !== null

  const { mutation: createManager, discard: discardCreateManager } =
    useSensitiveMutation({
      ...createRunnerManagerMutation(),
      mutationKey: SENSITIVE_MUTATION_KEYS.createRunnerManager,
    })
  const updateManager = useMutation({ ...updateRunnerManagerMutation() })

  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
    }
  }, [])

  const form = useForm({
    defaultValues: {
      name: manager?.name ?? '',
      endpoint: manager?.endpoint ?? DEFAULT_ENDPOINT,
      access_token: '',
      image: manager?.image ?? 'desk-foreman-workspace-runner:local',
      host_workspace_root: manager?.host_workspace_root ?? '',
      network_enabled: manager?.network_enabled ?? false,
      enabled: manager?.enabled ?? true,
      max_output_bytes: String(manager?.max_output_bytes ?? 262144),
      max_timeout_ms: String(manager?.max_timeout_ms ?? 600000),
      max_sessions: String(manager?.max_sessions ?? 32),
      pids_limit: String(manager?.pids_limit ?? 256),
      memory_limit: manager?.memory_limit ?? '1g',
      cpu_limit: manager?.cpu_limit ?? '2',
    },
    onSubmit: async ({ value }) => {
      setServerError('')
      const common = {
        endpoint: value.endpoint.trim(),
        image: value.image.trim(),
        host_workspace_root: value.host_workspace_root.trim() || null,
        network_enabled: value.network_enabled,
        max_output_bytes: toNumber(value.max_output_bytes, 262144),
        max_timeout_ms: toNumber(value.max_timeout_ms, 600000),
        max_sessions: toNumber(value.max_sessions, 32),
        pids_limit: toNumber(value.pids_limit, 256),
        memory_limit: value.memory_limit.trim() || '1g',
        cpu_limit: value.cpu_limit.trim() || '2',
      }
      const epoch = sessionEpoch
      submittingRef.current = true
      try {
        if (isEdit && manager) {
          const body: UpdateRunnerManagerRequest = {
            ...common,
            enabled: value.enabled,
          }
          await updateManager.mutateAsync({
            path: { runner_manager_id: manager.runner_manager_id },
            body,
          })
          if (!isSessionCurrent(epoch) || !mountedRef.current) return
          toast.success(t('runnerManagers.notifications.updated'), {
            description: value.name.trim(),
          })
        } else {
          const body: CreateRunnerManagerRequest = {
            ...common,
            name: value.name.trim(),
            access_token: value.access_token.trim() || null,
          }
          const result = await createManager.mutateAsync({ body })
          if (!isSessionCurrent(epoch) || !mountedRef.current) return
          toast.success(t('runnerManagers.notifications.registered'), {
            description: value.name.trim(),
          })
          if (result.token) onCreatedToken(result.token)
          form.reset()
          discardCreateManager()
        }
        await queryClient.invalidateQueries({
          queryKey: listRunnerManagersQueryKey(),
        })
        if (!isSessionCurrent(epoch) || !mountedRef.current) return
        onOpenChange(false)
      } catch (error) {
        if (!isSessionCurrent(epoch) || !mountedRef.current) return
        setServerError(errorMessage(error, t('runnerManagers.errors.save')))
        if (!isEdit) discardCreateManager()
      } finally {
        submittingRef.current = false
      }
    },
  })

  function close(): void {
    if (submittingRef.current) return
    form.reset()
    discardCreateManager()
    onOpenChange(false)
  }

  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) close()
      }}
    >
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>
            {isEdit
              ? t('runnerManagers.drawer.editTitle')
              : t('runnerManagers.drawer.createTitle')}
          </DialogTitle>
          <DialogDescription>
            {isEdit
              ? t('runnerManagers.drawer.editDescription')
              : t('runnerManagers.drawer.createDescription')}
          </DialogDescription>
        </DialogHeader>

        <form
          id="runner-manager-form"
          className="space-y-6"
          onSubmit={(event) => {
            event.preventDefault()
            event.stopPropagation()
            submittingRef.current = true
            void form.handleSubmit().finally(() => {
              submittingRef.current = false
            })
          }}
        >
          <div className="space-y-4">
            <div className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
              {t('runnerManagers.sections.connection')}
            </div>
            <FieldGroup>
              <form.Field
                name="name"
                validators={{
                  onBlur: ({ value }) =>
                    value.trim()
                      ? undefined
                      : t('runnerManagers.validation.nameRequired'),
                  onSubmit: ({ value }) =>
                    value.trim()
                      ? undefined
                      : t('runnerManagers.validation.nameRequired'),
                }}
              >
                {(field) => (
                  <Field
                    data-invalid={Boolean(firstError(field.state.meta.errors))}
                  >
                    <FieldLabel htmlFor="runner-manager-name">
                      {t('runnerManagers.fields.name')}
                    </FieldLabel>
                    <Input
                      id="runner-manager-name"
                      name="name"
                      autoComplete="off"
                      disabled={isEdit}
                      placeholder={t('runnerManagers.placeholders.name')}
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
                name="endpoint"
                validators={{
                  onBlur: ({ value }) =>
                    value.trim()
                      ? undefined
                      : t('runnerManagers.validation.endpointRequired'),
                  onSubmit: ({ value }) =>
                    value.trim()
                      ? undefined
                      : t('runnerManagers.validation.endpointRequired'),
                }}
              >
                {(field) => (
                  <Field
                    data-invalid={Boolean(firstError(field.state.meta.errors))}
                  >
                    <FieldLabel htmlFor="runner-manager-endpoint">
                      {t('runnerManagers.fields.endpoint')}
                    </FieldLabel>
                    <Input
                      id="runner-manager-endpoint"
                      name="endpoint"
                      autoComplete="off"
                      placeholder={t('runnerManagers.placeholders.endpoint')}
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

              <form.Field name="host_workspace_root">
                {(field) => (
                  <Field>
                    <FieldLabel htmlFor="runner-manager-host-root">
                      {t('runnerManagers.fields.hostWorkspaceRoot')}
                    </FieldLabel>
                    <Input
                      id="runner-manager-host-root"
                      name="host_workspace_root"
                      autoComplete="off"
                      placeholder={t(
                        'runnerManagers.placeholders.hostWorkspaceRoot',
                      )}
                      value={field.state.value}
                      onChange={(event) =>
                        field.handleChange(event.target.value)
                      }
                    />
                    <FieldDescription>
                      {t('runnerManagers.hints.hostWorkspaceRoot')}
                    </FieldDescription>
                  </Field>
                )}
              </form.Field>

              {!isEdit ? (
                <form.Field name="access_token">
                  {(field) => (
                    <Field>
                      <FieldLabel htmlFor="runner-manager-token">
                        {t('runnerManagers.fields.existingToken')}
                      </FieldLabel>
                      <Input
                        id="runner-manager-token"
                        name="access_token"
                        type="password"
                        autoComplete="off"
                        placeholder={t(
                          'runnerManagers.placeholders.existingToken',
                        )}
                        value={field.state.value}
                        onChange={(event) =>
                          field.handleChange(event.target.value)
                        }
                      />
                      <FieldDescription>
                        {t('runnerManagers.hints.existingToken')}
                      </FieldDescription>
                    </Field>
                  )}
                </form.Field>
              ) : null}

              {isEdit ? (
                <form.Field name="enabled">
                  {(field) => (
                    <div className="flex items-center justify-between gap-4 rounded-lg border p-3">
                      <div>
                        <div className="text-sm font-medium">
                          {t('runnerManagers.fields.enabled')}
                        </div>
                        <div className="text-xs text-muted-foreground">
                          {t('runnerManagers.hints.enabled')}
                        </div>
                      </div>
                      <Switch
                        checked={field.state.value}
                        aria-label={t('runnerManagers.fields.enabled')}
                        onCheckedChange={field.handleChange}
                      />
                    </div>
                  )}
                </form.Field>
              ) : null}
            </FieldGroup>
          </div>

          <div className="space-y-4">
            <div className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
              {t('runnerManagers.sections.container')}
            </div>
            <FieldGroup>
              <form.Field name="image">
                {(field) => (
                  <Field>
                    <FieldLabel htmlFor="runner-manager-image">
                      {t('runnerManagers.fields.runnerImage')}
                    </FieldLabel>
                    <Input
                      id="runner-manager-image"
                      name="image"
                      autoComplete="off"
                      placeholder={t('runnerManagers.placeholders.runnerImage')}
                      value={field.state.value}
                      onChange={(event) =>
                        field.handleChange(event.target.value)
                      }
                    />
                  </Field>
                )}
              </form.Field>

              <form.Field name="network_enabled">
                {(field) => (
                  <div className="flex items-center justify-between gap-4 rounded-lg border p-3">
                    <div>
                      <div className="text-sm font-medium">
                        {t('runnerManagers.fields.networkAccess')}
                      </div>
                      <div className="text-xs text-muted-foreground">
                        {t('runnerManagers.hints.networkAccess')}
                      </div>
                    </div>
                    <Switch
                      checked={field.state.value}
                      aria-label={t('runnerManagers.fields.networkAccess')}
                      onCheckedChange={field.handleChange}
                    />
                  </div>
                )}
              </form.Field>
              <p className="text-xs text-muted-foreground">
                {t('runnerManagers.hints.imageAndNetwork')}
              </p>
            </FieldGroup>
          </div>

          <div className="space-y-4">
            <div className="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
              {t('runnerManagers.sections.resourceLimits')}
            </div>
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              {(
                [
                  ['max_sessions', 'maxSessions', '1'],
                  ['max_output_bytes', 'maxOutput', '0'],
                  ['max_timeout_ms', 'maxTimeout', '0'],
                  ['pids_limit', 'pidLimit', '0'],
                ] as const
              ).map(([name, label, min]) => (
                <form.Field key={name} name={name}>
                  {(field) => (
                    <Field>
                      <FieldLabel htmlFor={`runner-manager-${name}`}>
                        {t(`runnerManagers.fields.${label}`)}
                      </FieldLabel>
                      <Input
                        id={`runner-manager-${name}`}
                        name={name}
                        type="number"
                        min={min}
                        value={field.state.value}
                        onChange={(event) =>
                          field.handleChange(event.target.value)
                        }
                      />
                    </Field>
                  )}
                </form.Field>
              ))}

              <form.Field name="memory_limit">
                {(field) => (
                  <Field>
                    <FieldLabel htmlFor="runner-manager-memory">
                      {t('runnerManagers.fields.memoryLimit')}
                    </FieldLabel>
                    <Input
                      id="runner-manager-memory"
                      name="memory_limit"
                      autoComplete="off"
                      placeholder={t('runnerManagers.placeholders.memoryLimit')}
                      value={field.state.value}
                      onChange={(event) =>
                        field.handleChange(event.target.value)
                      }
                    />
                  </Field>
                )}
              </form.Field>

              <form.Field name="cpu_limit">
                {(field) => (
                  <Field>
                    <FieldLabel htmlFor="runner-manager-cpu">
                      {t('runnerManagers.fields.cpuLimit')}
                    </FieldLabel>
                    <Input
                      id="runner-manager-cpu"
                      name="cpu_limit"
                      autoComplete="off"
                      placeholder={t('runnerManagers.placeholders.cpuLimit')}
                      value={field.state.value}
                      onChange={(event) =>
                        field.handleChange(event.target.value)
                      }
                    />
                  </Field>
                )}
              </form.Field>
            </div>
            <p className="text-xs text-muted-foreground">
              {t('runnerManagers.hints.limits')}
            </p>
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
            onClick={close}
          >
            {t('runnerManagers.actions.cancel')}
          </Button>
          <form.Subscribe
            selector={(state) => [state.canSubmit, state.isSubmitting] as const}
          >
            {([canSubmit, isSubmitting]) => (
              <Button
                type="submit"
                form="runner-manager-form"
                disabled={!canSubmit || isSubmitting}
              >
                {isEdit
                  ? t('runnerManagers.actions.save')
                  : t('runnerManagers.actions.register')}
              </Button>
            )}
          </form.Subscribe>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
