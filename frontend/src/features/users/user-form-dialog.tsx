import { useForm } from '@tanstack/react-form'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { CircleAlert } from 'lucide-react'
import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  createUserMutation,
  listUsersQueryKey,
  updateUserMutation,
  type CreateUserRequest,
  type UpdateUserRequest,
  type UserResponse,
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

const MINIMUM_PASSWORD_LENGTH = 8

function timezones(): string[] {
  try {
    return Intl.supportedValuesOf('timeZone')
  } catch {
    return ['UTC', 'Asia/Shanghai', 'Europe/London', 'America/New_York']
  }
}

function defaultTimezone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
}

interface UserFormDialogProps {
  user: UserResponse | null
  onOpenChange: (open: boolean) => void
}

export function UserFormDialog({ user, onOpenChange }: UserFormDialogProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const { sessionEpoch, isSessionCurrent } = useSession()
  const submittingRef = useRef(false)
  const [serverError, setServerError] = useState('')
  const isEdit = user !== null

  const { mutation: createUser, discard: discardCreateUser } =
    useSensitiveMutation({
      ...createUserMutation(),
      mutationKey: SENSITIVE_MUTATION_KEYS.createUser,
    })
  const updateUser = useMutation({ ...updateUserMutation() })

  const form = useForm({
    defaultValues: {
      login_name: user?.login_name ?? '',
      password: '',
      display_name: user?.display_name ?? '',
      email: user?.email ?? '',
      timezone: user?.timezone ?? defaultTimezone(),
      is_admin: user?.is_admin ?? false,
      is_active: user?.is_active ?? true,
    },
    onSubmit: async ({ value }) => {
      setServerError('')
      const epoch = sessionEpoch
      submittingRef.current = true
      try {
        if (isEdit && user) {
          const body: UpdateUserRequest = {
            display_name: value.display_name.trim(),
            email: value.email.trim(),
            timezone: value.timezone.trim(),
            is_admin: value.is_admin,
            is_active: value.is_active,
          }
          await updateUser.mutateAsync({
            path: { user_id: user.user_id },
            body,
          })
          if (!isSessionCurrent(epoch)) return
          toast.success(t('users.notifications.userUpdated'), {
            description: value.login_name,
          })
        } else {
          const body: CreateUserRequest = {
            login_name: value.login_name.trim(),
            password: value.password,
            display_name: value.display_name.trim(),
            email: value.email.trim(),
            timezone: value.timezone.trim(),
            is_admin: value.is_admin,
            workspace_root: null,
          }
          await createUser.mutateAsync({ body })
          if (!isSessionCurrent(epoch)) return
          form.reset()
          discardCreateUser()
          toast.success(t('users.notifications.userCreated'), {
            description: t('users.notifications.userCreatedDescription', {
              login: value.login_name.trim(),
            }),
          })
        }
        await queryClient.invalidateQueries({ queryKey: listUsersQueryKey() })
        if (!isSessionCurrent(epoch)) return
        onOpenChange(false)
      } catch (error) {
        if (!isSessionCurrent(epoch)) return
        setServerError(errorMessage(error, t('users.errors.save')))
        if (!isEdit) discardCreateUser()
      } finally {
        submittingRef.current = false
      }
    },
  })

  function close(): void {
    if (submittingRef.current) return
    form.reset()
    discardCreateUser()
    onOpenChange(false)
  }

  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) close()
      }}
    >
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>
            {isEdit
              ? t('users.drawer.editTitle')
              : t('users.drawer.createTitle')}
          </DialogTitle>
          <DialogDescription>
            {isEdit
              ? t('users.drawer.editDescription')
              : t('users.drawer.createDescription')}
          </DialogDescription>
        </DialogHeader>

        <form
          id="user-form"
          className="space-y-5"
          onSubmit={(event) => {
            event.preventDefault()
            event.stopPropagation()
            void form.handleSubmit()
          }}
        >
          <FieldGroup>
            <form.Field
              name="login_name"
              validators={{
                onBlur: ({ value }) =>
                  value.trim()
                    ? undefined
                    : t('users.validation.loginNameRequired'),
                onSubmit: ({ value }) =>
                  value.trim()
                    ? undefined
                    : t('users.validation.loginNameRequired'),
              }}
            >
              {(field) => (
                <Field
                  data-invalid={Boolean(firstError(field.state.meta.errors))}
                >
                  <FieldLabel htmlFor="user-login-name">
                    {t('users.fields.loginName')}
                  </FieldLabel>
                  <Input
                    id="user-login-name"
                    name="login_name"
                    autoComplete="off"
                    disabled={isEdit}
                    placeholder={t('users.placeholders.loginName')}
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

            {!isEdit ? (
              <form.Field
                name="password"
                validators={{
                  onBlur: ({ value }) => {
                    if (!value) return t('users.validation.passwordRequired')
                    if (value.length < MINIMUM_PASSWORD_LENGTH) {
                      return t('users.validation.passwordLength')
                    }
                    return undefined
                  },
                  onSubmit: ({ value }) => {
                    if (!value) return t('users.validation.passwordRequired')
                    if (value.length < MINIMUM_PASSWORD_LENGTH) {
                      return t('users.validation.passwordLength')
                    }
                    return undefined
                  },
                }}
              >
                {(field) => (
                  <Field
                    data-invalid={Boolean(firstError(field.state.meta.errors))}
                  >
                    <FieldLabel htmlFor="user-password">
                      {t('users.fields.password')}
                    </FieldLabel>
                    <Input
                      id="user-password"
                      name="password"
                      type="password"
                      autoComplete="new-password"
                      placeholder={t('users.placeholders.password')}
                      value={field.state.value}
                      aria-invalid={
                        firstError(field.state.meta.errors) ? true : undefined
                      }
                      onChange={(event) =>
                        field.handleChange(event.target.value)
                      }
                      onBlur={field.handleBlur}
                    />
                    <FieldDescription>
                      {t('users.hints.password')}
                    </FieldDescription>
                    <FieldError>
                      {firstError(field.state.meta.errors)}
                    </FieldError>
                  </Field>
                )}
              </form.Field>
            ) : null}

            <form.Field
              name="display_name"
              validators={{
                onBlur: ({ value }) =>
                  value.trim()
                    ? undefined
                    : t('users.validation.displayNameRequired'),
                onSubmit: ({ value }) =>
                  value.trim()
                    ? undefined
                    : t('users.validation.displayNameRequired'),
              }}
            >
              {(field) => (
                <Field
                  data-invalid={Boolean(firstError(field.state.meta.errors))}
                >
                  <FieldLabel htmlFor="user-display-name">
                    {t('users.fields.displayName')}
                  </FieldLabel>
                  <Input
                    id="user-display-name"
                    name="display_name"
                    autoComplete="off"
                    placeholder={t('users.placeholders.displayName')}
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
              name="email"
              validators={{
                onBlur: ({ value }) =>
                  value.trim()
                    ? undefined
                    : t('users.validation.emailRequired'),
                onSubmit: ({ value }) =>
                  value.trim()
                    ? undefined
                    : t('users.validation.emailRequired'),
              }}
            >
              {(field) => (
                <Field
                  data-invalid={Boolean(firstError(field.state.meta.errors))}
                >
                  <FieldLabel htmlFor="user-email">
                    {t('users.fields.email')}
                  </FieldLabel>
                  <Input
                    id="user-email"
                    name="email"
                    type="email"
                    autoComplete="off"
                    placeholder={t('users.placeholders.email')}
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

            <form.Field name="timezone">
              {(field) => (
                <Field>
                  <FieldLabel htmlFor="user-timezone">
                    {t('users.fields.timezone')}
                  </FieldLabel>
                  <Input
                    id="user-timezone"
                    name="timezone"
                    list="user-timezone-options"
                    autoComplete="off"
                    value={field.state.value}
                    onChange={(event) => field.handleChange(event.target.value)}
                    onBlur={field.handleBlur}
                  />
                  <datalist id="user-timezone-options">
                    {timezones().map((zone) => (
                      <option key={zone} value={zone} />
                    ))}
                  </datalist>
                </Field>
              )}
            </form.Field>

            {isEdit && user ? (
              <Field>
                <FieldLabel htmlFor="user-workspace-root">
                  {t('users.fields.workspaceRoot')}
                </FieldLabel>
                <Input
                  id="user-workspace-root"
                  value={user.workspace_root}
                  readOnly
                />
                <FieldDescription>
                  {t('users.hints.workspaceRoot')}
                </FieldDescription>
              </Field>
            ) : null}

            <div className="space-y-3 rounded-lg border p-4">
              <form.Field name="is_admin">
                {(field) => (
                  <div className="flex items-center justify-between gap-4">
                    <div>
                      <div className="text-sm font-medium">
                        {t('users.fields.administrator')}
                      </div>
                      <div className="text-xs text-muted-foreground">
                        {t('users.hints.administrator')}
                      </div>
                    </div>
                    <Switch
                      checked={field.state.value}
                      aria-label={t('users.fields.administrator')}
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
                          {t('users.fields.active')}
                        </div>
                        <div className="text-xs text-muted-foreground">
                          {t('users.hints.active')}
                        </div>
                      </div>
                      <Switch
                        checked={field.state.value}
                        aria-label={t('users.fields.active')}
                        onCheckedChange={field.handleChange}
                      />
                    </div>
                  )}
                </form.Field>
              ) : null}
            </div>
          </FieldGroup>

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
            {t('shared.confirm.cancel')}
          </Button>
          <form.Subscribe
            selector={(state) => [state.canSubmit, state.isSubmitting] as const}
          >
            {([canSubmit, isSubmitting]) => (
              <Button
                type="submit"
                form="user-form"
                disabled={!canSubmit || isSubmitting}
              >
                {isEdit
                  ? t('users.actions.saveChanges')
                  : t('users.actions.create')}
              </Button>
            )}
          </form.Subscribe>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
