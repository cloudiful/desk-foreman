import { AlertCircle } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'

interface ErrorAlertProps {
  error: string
  onRetry?: () => void
}

export function ErrorAlert({ error, onRetry }: ErrorAlertProps) {
  const { t } = useTranslation()

  return (
    <Alert variant="destructive">
      <AlertCircle aria-hidden="true" />
      <AlertTitle>{error}</AlertTitle>
      <AlertDescription className="flex flex-wrap items-center justify-between gap-3">
        <span>{t('shared.errorAlert.description')}</span>
        {onRetry ? (
          <Button size="sm" variant="outline" onClick={onRetry}>
            {t('shared.errorAlert.retry')}
          </Button>
        ) : null}
      </AlertDescription>
    </Alert>
  )
}
