import { useForm } from '@tanstack/react-form'
import { CircleAlert, KeyRound } from 'lucide-react'
import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { resetUserPasswordMutation, type UserResponse } from '@/api/generated'
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
import { Field, FieldError, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'

const MINIMUM_PASSWORD_LENGTH = 8

interface ResetPasswordDialogProps {
  user: UserResponse
  onOpenChange: (open: boolean) => void
}

export function ResetPasswordDialog({
  user,
  onOpenChange,
}: ResetPasswordDialogProps) {
  const { t } = useTranslation()
  const { sessionEpoch, isSessionCurrent } = useSession()
  const submittingRef = useRef(false)
  const [serverError, setServerError] = useState('')
  const { mutation: resetPassword, discard: discardResetPassword } =
    useSensitiveMutation({
      ...resetUserPasswordMutation(),
      mutationKey: SENSITIVE_MUTATION_KEYS.resetUserPassword,
    })

  const form = useForm({
    defaultValues: { password: '' },
    onSubmit: async ({ value }) => {
      setServerError('')
      const epoch = sessionEpoch
      submittingRef.current = true
      try {
        await resetPassword.mutateAsync({
          path: { user_id: user.user_id },
          body: { password: value.password },
        })
        if (!isSessionCurrent(epoch)) return
        form.reset()
        discardResetPassword()
        toast.success(t('users.notifications.passwordReset'), {
          description: user.login_name,
        })
        onOpenChange(false)
      } catch (error) {
        if (!isSessionCurrent(epoch)) return
        setServerError(errorMessage(error, t('users.errors.reset')))
        discardResetPassword()
      } finally {
        submittingRef.current = false
      }
    },
  })

  function close(): void {
    if (submittingRef.current) return
    form.reset()
    discardResetPassword()
    onOpenChange(false)
  }

  return (
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next) close()
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t('users.confirmations.resetTitle')}</DialogTitle>
          <DialogDescription>
            {t('users.confirmations.resetDescription', {
              displayName: user.display_name,
              login: user.login_name,
            })}
          </DialogDescription>
        </DialogHeader>

        <form
          id="reset-password-form"
          onSubmit={(event) => {
            event.preventDefault()
            event.stopPropagation()
            void form.handleSubmit()
          }}
        >
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
                <FieldLabel htmlFor="reset-password">
                  {t('users.fields.password')}
                </FieldLabel>
                <Input
                  id="reset-password"
                  name="password"
                  type="password"
                  autoComplete="new-password"
                  placeholder={t('users.placeholders.newPassword')}
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

          {serverError ? (
            <Alert variant="destructive" className="mt-4">
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
                form="reset-password-form"
                disabled={!canSubmit || isSubmitting}
              >
                <KeyRound aria-hidden="true" />
                {t('users.actions.resetPassword')}
              </Button>
            )}
          </form.Subscribe>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
