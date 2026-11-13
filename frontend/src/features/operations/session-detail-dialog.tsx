import { useTranslation } from 'react-i18next'
import type { RunnerSessionResponse } from '@/api/generated'
import { StatusBadge } from '@/components/status-badge'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { formatDuration } from '@/utils/format'

interface SessionDetailDialogProps {
  session: RunnerSessionResponse | null
  onClose: () => void
}

export function SessionDetailDialog({
  session,
  onClose,
}: SessionDetailDialogProps) {
  const { t } = useTranslation()

  return (
    <Dialog open={session !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{t('operations.sessions.detailsTitle')}</DialogTitle>
          <DialogDescription>
            {session ? `#${session.session_id}` : null}
          </DialogDescription>
        </DialogHeader>
        {session ? (
          <dl className="space-y-3 text-sm">
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.sessions.details.sessionId')}
              </dt>
              <dd className="font-mono">#{session.session_id}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.sessions.details.sessionKey')}
              </dt>
              <dd className="max-w-[55%] truncate text-right font-mono">
                {session.session_key ?? '—'}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.sessions.details.owner')}
              </dt>
              <dd>
                {session.owner_kind} #{session.owner_id}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.sessions.details.state')}
              </dt>
              <dd>
                <StatusBadge status={session.state} />
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.sessions.details.exitCode')}
              </dt>
              <dd className="font-mono">{session.exit_code ?? '—'}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.sessions.details.wallTime')}
              </dt>
              <dd>{formatDuration(session.wall_time_seconds)}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.sessions.details.timedOut')}
              </dt>
              <dd>
                {session.timed_out ? t('operations.yes') : t('operations.no')}
              </dd>
            </div>
          </dl>
        ) : null}
      </DialogContent>
    </Dialog>
  )
}
