import { Compass } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Card, CardContent } from '@/components/ui/card'

export function NotFoundPage() {
  const { t } = useTranslation()

  return (
    <div className="flex min-h-screen items-center justify-center bg-muted px-4">
      <Card className="w-full max-w-sm">
        <CardContent className="flex flex-col items-start gap-2 py-8">
          <Compass
            className="size-5 text-muted-foreground"
            aria-hidden="true"
          />
          <h1 className="text-sm font-semibold">{t('shell.notFound.title')}</h1>
          <p className="text-sm text-muted-foreground">
            {t('shell.notFound.description')}
          </p>
        </CardContent>
      </Card>
    </div>
  )
}
