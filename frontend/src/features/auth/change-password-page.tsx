import { useForm } from '@tanstack/react-form'
import { Check, CircleAlert, KeyRound } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
} from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { errorMessage } from '@/api/http'
import { useSession } from './session-context'

const MINIMUM_PASSWORD_LENGTH = 8

function firstError(errors: readonly unknown[]): string | undefined {
  const error = errors[0]
  return typeof error === 'string' && error ? error : undefined
}

export function ChangePasswordPage() {
  const { t } = useTranslation()
  const { changePassword, user } = useSession()
  const navigate = useNavigate()
  const [serverError, setServerError] = useState('')

  const form = useForm({
    defaultValues: {
      current_password: '',
      new_password: '',
      confirm_password: '',
    },
    onSubmit: async ({ value }) => {
      setServerError('')
      try {
        await changePassword({
          current_password: value.current_password,
          new_password: value.new_password,
        })
        form.reset()
        navigate('/', { replace: true })
      } catch (error) {
        setServerError(
          errorMessage(error, t('auth.errors.changePasswordFailed')),
        )
      }
    },
  })

  return (
    <div className="flex min-h-screen items-center justify-center bg-muted px-4">
      <div className="w-full max-w-sm">
        <div className="mb-8 flex flex-col items-center gap-3">
          <div className="flex size-12 items-center justify-center rounded-xl bg-primary text-primary-foreground shadow-lg">
            <KeyRound className="size-6" aria-hidden="true" />
          </div>
          <div className="text-center">
            <h1 className="text-xl font-semibold tracking-tight">
              {t('auth.changePassword.title')}
            </h1>
            <p className="mt-1 text-sm text-muted-foreground">
              {user?.must_change_password
                ? t('auth.changePassword.subtitle')
                : t('auth.changePassword.voluntarySubtitle')}
            </p>
          </div>
        </div>

        <Card>
          <CardContent>
            <form
              className="grid gap-4"
              onSubmit={(event) => {
                event.preventDefault()
                event.stopPropagation()
                void form.handleSubmit()
              }}
            >
              <FieldGroup>
                <form.Field
                  name="current_password"
                  validators={{
                    onBlur: ({ value }) =>
                      value
                        ? undefined
                        : t('auth.changePassword.currentPasswordRequired'),
                  }}
                >
                  {(field) => (
                    <Field
                      data-invalid={Boolean(
                        firstError(field.state.meta.errors),
                      )}
                    >
                      <FieldLabel htmlFor="current_password">
                        {t('auth.changePassword.currentPassword')}
                      </FieldLabel>
                      <Input
                        id="current_password"
                        name="current_password"
                        type="password"
                        autoComplete="current-password"
                        placeholder={t('auth.login.passwordPlaceholder')}
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
                  name="new_password"
                  validators={{
                    onBlur: ({ value }) =>
                      value.length >= MINIMUM_PASSWORD_LENGTH
                        ? undefined
                        : t('auth.changePassword.minimumLength'),
                  }}
                >
                  {(field) => (
                    <Field
                      data-invalid={Boolean(
                        firstError(field.state.meta.errors),
                      )}
                    >
                      <FieldLabel htmlFor="new_password">
                        {t('auth.changePassword.newPassword')}
                      </FieldLabel>
                      <Input
                        id="new_password"
                        name="new_password"
                        type="password"
                        autoComplete="new-password"
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
                        {t('auth.changePassword.newPasswordHint')}
                      </FieldDescription>
                      <FieldError>
                        {firstError(field.state.meta.errors)}
                      </FieldError>
                    </Field>
                  )}
                </form.Field>

                <form.Field
                  name="confirm_password"
                  validators={{
                    onBlur: ({ value, fieldApi }) => {
                      const newPassword =
                        fieldApi.form.getFieldValue('new_password')
                      if (!value) return t('auth.login.required')
                      if (value !== newPassword) {
                        return t('auth.changePassword.mismatch')
                      }
                      return undefined
                    },
                  }}
                >
                  {(field) => (
                    <Field
                      data-invalid={Boolean(
                        firstError(field.state.meta.errors),
                      )}
                    >
                      <FieldLabel htmlFor="confirm_password">
                        {t('auth.changePassword.confirmPassword')}
                      </FieldLabel>
                      <Input
                        id="confirm_password"
                        name="confirm_password"
                        type="password"
                        autoComplete="new-password"
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
              </FieldGroup>

              {serverError ? (
                <Alert variant="destructive">
                  <CircleAlert aria-hidden="true" />
                  <AlertDescription>{serverError}</AlertDescription>
                </Alert>
              ) : null}

              <form.Subscribe
                selector={(state) =>
                  [state.canSubmit, state.isSubmitting] as const
                }
              >
                {([canSubmit, isSubmitting]) => (
                  <Button type="submit" size="lg" disabled={!canSubmit}>
                    <Check aria-hidden="true" />
                    {isSubmitting
                      ? t('shared.dataTable.loading')
                      : t('auth.changePassword.changePassword')}
                  </Button>
                )}
              </form.Subscribe>
            </form>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
