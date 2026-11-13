import { useForm } from '@tanstack/react-form'
import { CircleAlert, Hammer, LogIn } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useLocation, useNavigate } from 'react-router'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import {
  Field,
  FieldError,
  FieldGroup,
  FieldLabel,
} from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { errorMessage } from '@/api/http'
import { useSession } from './session-context'

function firstError(errors: readonly unknown[]): string | undefined {
  const error = errors[0]
  return typeof error === 'string' && error ? error : undefined
}

export function LoginPage() {
  const { t } = useTranslation()
  const { login } = useSession()
  const navigate = useNavigate()
  const location = useLocation()
  const [serverError, setServerError] = useState('')

  const destination =
    typeof location.state === 'object' &&
    location.state !== null &&
    'from' in location.state &&
    typeof location.state.from === 'string'
      ? location.state.from
      : '/'

  const form = useForm({
    defaultValues: { login_name: '', password: '' },
    onSubmit: async ({ value }) => {
      setServerError('')
      try {
        await login({ login_name: value.login_name, password: value.password })
        form.reset()
        navigate(destination, { replace: true })
      } catch (error) {
        setServerError(errorMessage(error, t('auth.errors.loginFailed')))
      }
    },
  })

  return (
    <div className="flex min-h-screen items-center justify-center bg-muted px-4">
      <div className="w-full max-w-sm">
        <div className="mb-8 flex flex-col items-center gap-3">
          <div className="flex size-12 items-center justify-center rounded-xl bg-primary text-primary-foreground shadow-lg">
            <Hammer className="size-6" aria-hidden="true" />
          </div>
          <div className="text-center">
            <h1 className="text-xl font-semibold tracking-tight">
              {t('shell.productName')}
            </h1>
            <p className="mt-1 text-sm text-muted-foreground">
              {t('auth.login.subtitle')}
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
                  name="login_name"
                  validators={{
                    onBlur: ({ value }) =>
                      value.trim() ? undefined : t('auth.login.required'),
                  }}
                >
                  {(field) => (
                    <Field
                      data-invalid={Boolean(
                        firstError(field.state.meta.errors),
                      )}
                    >
                      <FieldLabel htmlFor="login_name">
                        {t('auth.login.loginName')}
                      </FieldLabel>
                      <Input
                        id="login_name"
                        name="login_name"
                        autoComplete="username"
                        placeholder={t('auth.login.loginNamePlaceholder')}
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
                  name="password"
                  validators={{
                    onBlur: ({ value }) =>
                      value ? undefined : t('auth.login.required'),
                  }}
                >
                  {(field) => (
                    <Field
                      data-invalid={Boolean(
                        firstError(field.state.meta.errors),
                      )}
                    >
                      <FieldLabel htmlFor="password">
                        {t('auth.login.password')}
                      </FieldLabel>
                      <Input
                        id="password"
                        name="password"
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
                    <LogIn aria-hidden="true" />
                    {isSubmitting
                      ? t('shared.dataTable.loading')
                      : t('auth.login.signIn')}
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
