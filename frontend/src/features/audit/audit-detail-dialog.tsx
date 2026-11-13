import { useTranslation } from 'react-i18next'
import type { AuditLogResponse } from '@/api/generated'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { formatDateTime, formatMilliseconds } from '@/utils/format'

function payloadPreview(payload: unknown): string {
  if (payload === null || payload === undefined) return ''
  try {
    return JSON.stringify(payload)
  } catch {
    return String(payload)
  }
}

interface AuditDetailDialogProps {
  event: AuditLogResponse | null
  onClose: () => void
}

export function AuditDetailDialog({ event, onClose }: AuditDetailDialogProps) {
  const { t } = useTranslation()

  return (
    <Dialog open={event !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{t('audit.detailTitle')}</DialogTitle>
          <DialogDescription>
            {event ? formatDateTime(event.created_at) : null}
          </DialogDescription>
        </DialogHeader>
        {event ? (
          <dl className="space-y-3 text-sm">
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('audit.detail.time')}
              </dt>
              <dd>{formatDateTime(event.created_at)}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('audit.detail.action')}
              </dt>
              <dd>
                <code className="font-mono">{event.action}</code>
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('audit.detail.actor')}
              </dt>
              <dd>
                {event.actor_type}
                {event.actor_user_id ? (
                  <span className="font-mono text-xs">
                    #{event.actor_user_id}
                  </span>
                ) : null}
                {event.actor_application_id ? (
                  <span className="font-mono text-xs">
                    #{event.actor_application_id}
                  </span>
                ) : null}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('audit.detail.target')}
              </dt>
              <dd className="max-w-[55%] break-all text-right font-mono">
                {event.target_type} {event.target_id}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('audit.detail.outcome')}
              </dt>
              <dd>{event.status ?? t('audit.notAvailable')}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('audit.detail.duration')}
              </dt>
              <dd>{formatMilliseconds(event.duration_ms)}</dd>
            </div>
            {event.request_id ? (
              <div className="flex justify-between gap-4">
                <dt className="text-muted-foreground">
                  {t('audit.detail.requestId')}
                </dt>
                <dd className="max-w-[55%] break-all text-right font-mono text-xs">
                  {event.request_id}
                </dd>
              </div>
            ) : null}
            <div className="rounded-lg border bg-muted/40 p-3">
              <dt className="mb-2 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
                {t('audit.detail.payload')}
              </dt>
              <dd>
                <pre className="max-h-72 overflow-auto font-mono text-xs leading-relaxed whitespace-pre-wrap">
                  {payloadPreview(event.payload)}
                </pre>
              </dd>
            </div>
          </dl>
        ) : null}
      </DialogContent>
    </Dialog>
  )
}
