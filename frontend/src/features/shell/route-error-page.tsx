import { CircleAlert } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'

export function RouteErrorPage() {
  const { t } = useTranslation()

  return (
    <div className="flex min-h-screen items-center justify-center bg-muted px-4">
      <Alert variant="destructive" className="max-w-md">
        <CircleAlert aria-hidden="true" />
        <AlertTitle>{t('app.errorBoundary.title')}</AlertTitle>
        <AlertDescription>
          <span>{t('app.errorBoundary.description')}</span>
          <Button
            variant="destructive"
            size="sm"
            onClick={() => window.location.reload()}
          >
            {t('app.errorBoundary.reload')}
          </Button>
        </AlertDescription>
      </Alert>
    </div>
  )
}
