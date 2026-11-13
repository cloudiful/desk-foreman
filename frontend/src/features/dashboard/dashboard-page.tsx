import { useQuery } from '@tanstack/react-query'
import {
  AppWindow,
  PlayCircle,
  RefreshCw,
  ServerCog,
  TerminalSquare,
  TriangleAlert,
  UserPlus,
  Archive,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router'
import {
  listRunnerManagersOptions,
  operationsSummaryOptions,
  type RunnerManagerResponse,
} from '@/api/generated'
import { errorMessage } from '@/api/http'
import { EmptyState } from '@/components/empty-state'
import { ErrorAlert } from '@/components/error-alert'
import { PageHeader } from '@/components/page-header'
import { StatCard } from '@/components/stat-card'
import { StatusBadge } from '@/components/status-badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { formatRelative } from '@/utils/format'

function managerStatus(manager: RunnerManagerResponse): string {
  if (!manager.enabled) return 'disabled'
  return manager.status === 'online' ? 'online' : 'offline'
}

export function DashboardPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()

  const summaryQuery = useQuery({
    ...operationsSummaryOptions(),
    staleTime: 15_000,
  })

  const managersQuery = useQuery({
    ...listRunnerManagersOptions({ query: { limit: 5 } }),
    staleTime: 15_000,
  })

  const summary = summaryQuery.data
  const managers = managersQuery.data?.items ?? []

  const loading = summaryQuery.isPending || managersQuery.isPending

  function refresh(): void {
    void summaryQuery.refetch()
    void managersQuery.refetch()
  }

  return (
    <div className="space-y-6">
      <PageHeader
        actions={
          <Button
            variant="outline"
            disabled={loading}
            aria-label={t('dashboard.refresh')}
            onClick={refresh}
          >
            <RefreshCw aria-hidden="true" />
            {t('dashboard.refresh')}
          </Button>
        }
      />

      {summaryQuery.error ? (
        <ErrorAlert
          error={errorMessage(summaryQuery.error, t('shared.requestFailed'))}
          onRetry={refresh}
        />
      ) : null}

      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
        <StatCard
          title={t('dashboard.stats.activeRunners')}
          value={summary?.active_runners ?? 0}
          icon={PlayCircle}
          loading={summaryQuery.isPending}
        />
        <StatCard
          title={t('dashboard.stats.activeSessions')}
          value={summary?.active_sessions ?? 0}
          icon={TerminalSquare}
          loading={summaryQuery.isPending}
        />
        <StatCard
          title={t('dashboard.stats.failedOperations')}
          value={summary?.failed_operations ?? 0}
          icon={TriangleAlert}
          loading={summaryQuery.isPending}
        />
        <StatCard
          title={t('dashboard.stats.archivedWorkspaces')}
          value={summary?.archived_workspaces ?? 0}
          icon={Archive}
          loading={summaryQuery.isPending}
        />
      </div>

      <div className="grid gap-6 lg:grid-cols-5">
        <Card className="lg:col-span-3">
          <CardHeader className="flex flex-row items-center justify-between gap-3 border-b">
            <div>
              <CardTitle className="text-sm">
                {t('dashboard.runnerManagers')}
              </CardTitle>
              <p className="mt-1 text-xs text-muted-foreground">
                {t('dashboard.runnerManagersSummary', {
                  online: summary?.runner_managers_online ?? 0,
                  offline: summary?.runner_managers_offline ?? 0,
                  disabled: summary?.runner_managers_disabled ?? 0,
                })}
              </p>
            </div>
            <Button
              size="sm"
              onClick={() => navigate('/admin/runner-managers')}
            >
              {t('dashboard.manage')}
            </Button>
          </CardHeader>
          <CardContent className="p-0">
            {managersQuery.error ? (
              <div className="p-4">
                <ErrorAlert
                  error={errorMessage(
                    managersQuery.error,
                    t('shared.requestFailed'),
                  )}
                  onRetry={() => void managersQuery.refetch()}
                />
              </div>
            ) : managersQuery.isPending ? (
              <ul className="divide-y">
                {Array.from({ length: 3 }, (_, index) => (
                  <li key={index} className="px-5 py-3.5">
                    <div className="h-4 w-40 animate-pulse rounded bg-muted" />
                    <div className="mt-2 h-3 w-56 animate-pulse rounded bg-muted" />
                  </li>
                ))}
              </ul>
            ) : managers.length === 0 ? (
              <EmptyState
                title={t('dashboard.noRunnerManagers')}
                description={t('dashboard.noRunnerManagersDescription')}
              />
            ) : (
              <ul className="divide-y">
                {managers.map((manager) => (
                  <li
                    key={manager.runner_manager_id}
                    className="flex items-center justify-between gap-3 px-5 py-3.5"
                  >
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="truncate text-sm font-medium">
                          {manager.name}
                        </span>
                        <StatusBadge status={managerStatus(manager)} />
                      </div>
                      <p className="mt-0.5 truncate font-mono text-xs text-muted-foreground">
                        {manager.endpoint}
                      </p>
                    </div>
                    <div className="shrink-0 text-right text-xs text-muted-foreground">
                      {t('dashboard.lastSeen', {
                        time: formatRelative(manager.last_seen_at),
                      })}
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </CardContent>
        </Card>

        <Card className="lg:col-span-2">
          <CardHeader>
            <CardTitle className="text-sm">
              {t('dashboard.quickActions')}
            </CardTitle>
            <p className="mt-1 text-xs text-muted-foreground">
              {t('dashboard.commonTasks')}
            </p>
          </CardHeader>
          <CardContent className="space-y-2">
            <Button
              variant="secondary"
              className="w-full justify-start"
              onClick={() => navigate('/admin/users')}
            >
              <UserPlus aria-hidden="true" />
              {t('dashboard.createUser')}
            </Button>
            <Button
              variant="secondary"
              className="w-full justify-start"
              onClick={() => navigate('/admin/applications')}
            >
              <AppWindow aria-hidden="true" />
              {t('dashboard.createApplication')}
            </Button>
            <Button
              variant="secondary"
              className="w-full justify-start"
              onClick={() => navigate('/admin/runner-managers')}
            >
              <ServerCog aria-hidden="true" />
              {t('dashboard.registerRunnerManager')}
            </Button>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
