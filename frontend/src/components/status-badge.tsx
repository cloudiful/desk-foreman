import { useTranslation } from 'react-i18next'
import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'

type StatusTone = 'success' | 'error' | 'warning' | 'neutral'

const TONE_CLASSES: Record<StatusTone, string> = {
  success:
    'border-transparent bg-emerald-500/15 text-emerald-700 dark:text-emerald-400',
  error: 'border-transparent bg-destructive/15 text-destructive',
  warning:
    'border-transparent bg-amber-500/15 text-amber-700 dark:text-amber-400',
  neutral: 'border-transparent bg-secondary text-secondary-foreground',
}

const STATUS_TONES: Record<string, StatusTone> = {
  online: 'success',
  active: 'success',
  success: 'success',
  running: 'success',
  ready: 'success',
  offline: 'error',
  inactive: 'error',
  failed: 'error',
  failure: 'error',
  error: 'error',
  cleanup_failed: 'error',
  timed_out: 'error',
  pending: 'warning',
  idle: 'warning',
  stale: 'warning',
  archived: 'warning',
  expired: 'warning',
  cancelled: 'neutral',
  exited: 'neutral',
  removed: 'neutral',
  disabled: 'neutral',
}

interface StatusBadgeProps {
  status: string | null | undefined
}

export function StatusBadge({ status }: StatusBadgeProps) {
  const { t } = useTranslation()
  const key = status ?? ''
  const tone = STATUS_TONES[key] ?? 'neutral'
  const label = key
    ? t(`shared.status.${key}`, { defaultValue: key })
    : t('shared.status.unknown')

  return (
    <Badge className={cn('gap-1.5', TONE_CLASSES[tone])}>
      <span className="size-1.5 rounded-full bg-current" aria-hidden="true" />
      <span className="whitespace-nowrap">{label}</span>
    </Badge>
  )
}
