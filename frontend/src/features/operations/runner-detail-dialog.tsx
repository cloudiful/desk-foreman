import { useTranslation } from 'react-i18next'
import type { WorkspaceRunnerResponse } from '@/api/generated'
import { StatusBadge } from '@/components/status-badge'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { formatDateTime } from '@/utils/format'

interface RunnerDetailDialogProps {
  runner: WorkspaceRunnerResponse | null
  onClose: () => void
}

export function RunnerDetailDialog({
  runner,
  onClose,
}: RunnerDetailDialogProps) {
  const { t } = useTranslation()

  return (
    <Dialog open={runner !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{t('operations.runners.detailsTitle')}</DialogTitle>
          <DialogDescription>
            {runner ? runner.container_name : null}
          </DialogDescription>
        </DialogHeader>
        {runner ? (
          <dl className="space-y-3 text-sm">
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.runners.details.container')}
              </dt>
              <dd className="max-w-[55%] truncate text-right font-mono">
                {runner.container_name}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.runners.details.containerId')}
              </dt>
              <dd className="max-w-[55%] break-all text-right font-mono">
                {runner.container_id ?? '—'}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.runners.details.runnerManager')}
              </dt>
              <dd>{runner.runner_manager_id ?? '—'}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.runners.details.status')}
              </dt>
              <dd>
                <StatusBadge status={runner.status} />
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.runners.details.runtime')}
              </dt>
              <dd>{runner.runtime}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.runners.details.image')}
              </dt>
              <dd className="max-w-[55%] truncate text-right font-mono">
                {runner.image_name}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.runners.details.workspaceRoot')}
              </dt>
              <dd className="max-w-[55%] truncate text-right font-mono">
                {runner.workspace_root}
              </dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.runners.details.lastActive')}
              </dt>
              <dd>{formatDateTime(runner.last_active_at)}</dd>
            </div>
            <div className="flex justify-between gap-4">
              <dt className="text-muted-foreground">
                {t('operations.runners.details.lastObserved')}
              </dt>
              <dd>{formatDateTime(runner.last_observed_at)}</dd>
            </div>
            {runner.last_error ? (
              <div className="rounded-lg border border-destructive/40 bg-muted/40 p-3">
                <dt className="mb-1 text-xs font-semibold tracking-wide text-destructive uppercase">
                  {t('operations.runners.details.lastError')}
                </dt>
                <dd className="font-mono text-xs whitespace-pre-wrap text-muted-foreground">
                  {runner.last_error}
                </dd>
              </div>
            ) : null}
          </dl>
        ) : null}
      </DialogContent>
    </Dialog>
  )
}
