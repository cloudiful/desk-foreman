import { Loader2 } from 'lucide-react'
import { useTranslation } from 'react-i18next'

export function FullPageLoading({ label }: { label?: string }) {
  const { t } = useTranslation()

  return (
    <div className="flex min-h-screen items-center justify-center gap-2 text-muted-foreground">
      <Loader2 className="size-4 animate-spin" aria-hidden="true" />
      <span className="text-sm">{label ?? t('shared.dataTable.loading')}</span>
    </div>
  )
}
