import { useQuery } from '@tanstack/react-query'
import type { ColumnDef } from '@tanstack/react-table'
import {
  Eye,
  Radar,
  RefreshCw,
  PlayCircle,
  TerminalSquare,
  TriangleAlert,
} from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  listRunnerSessionsOptions,
  listWorkspaceRunnersOptions,
  operationsSummaryOptions,
  type RunnerSessionResponse,
  type WorkspaceRunnerResponse,
} from '@/api/generated'
import { errorMessage } from '@/api/http'
import { DataTable } from '@/components/data-table'
import { ErrorAlert } from '@/components/error-alert'
import { PageHeader } from '@/components/page-header'
import { StatCard } from '@/components/stat-card'
import { StatusBadge } from '@/components/status-badge'
import { TablePagination } from '@/components/table-pagination'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { useDocumentVisibility } from '@/hooks/use-document-visibility'
import { PAGE_SIZE, pageCount, pageOffset } from '@/utils/pagination'
import { formatDateTime, formatDuration } from '@/utils/format'
import { RunnerDetailDialog } from './runner-detail-dialog'
import { SessionDetailDialog } from './session-detail-dialog'

const AUTO_REFRESH_KEY = 'desk-foreman-operations-auto-refresh'
const AUTO_REFRESH_INTERVAL_MS = 15_000

function readAutoRefresh(): boolean {
  try {
    return window.localStorage.getItem(AUTO_REFRESH_KEY) !== 'false'
  } catch {
    return true
  }
}

export function OperationsPage() {
  const { t } = useTranslation()
  const [autoRefresh, setAutoRefresh] = useState(readAutoRefresh)
  const [runnersPage, setRunnersPage] = useState(1)
  const [sessionsPage, setSessionsPage] = useState(1)
  const [activeTab, setActiveTab] = useState<'runners' | 'sessions'>('runners')
  const [runnerDetail, setRunnerDetail] =
    useState<WorkspaceRunnerResponse | null>(null)
  const [sessionDetail, setSessionDetail] =
    useState<RunnerSessionResponse | null>(null)

  const hidden = useDocumentVisibility()
  const wasHidden = useRef(hidden)
  const refetchInterval =
    autoRefresh && !hidden ? AUTO_REFRESH_INTERVAL_MS : false

  const runnersQuery = useQuery({
    ...listWorkspaceRunnersOptions({
      query: {
        limit: PAGE_SIZE,
        offset: pageOffset(runnersPage, PAGE_SIZE),
      },
    }),
    refetchInterval,
    refetchIntervalInBackground: false,
    placeholderData: (previous) => previous,
  })

  const sessionsQuery = useQuery({
    ...listRunnerSessionsOptions({
      query: {
        limit: PAGE_SIZE,
        offset: pageOffset(sessionsPage, PAGE_SIZE),
      },
    }),
    refetchInterval,
    refetchIntervalInBackground: false,
    placeholderData: (previous) => previous,
  })

  const summaryQuery = useQuery({
    ...operationsSummaryOptions(),
    refetchInterval,
    refetchIntervalInBackground: false,
  })

  const refresh = (): void => {
    void runnersQuery.refetch()
    void sessionsQuery.refetch()
    void summaryQuery.refetch()
  }

  const refreshRef = useRef(refresh)
  refreshRef.current = refresh

  useEffect(() => {
    if (wasHidden.current && !hidden) refreshRef.current()
    wasHidden.current = hidden
  }, [hidden])

  function toggleAutoRefresh(value: boolean): void {
    setAutoRefresh(value)
    try {
      window.localStorage.setItem(AUTO_REFRESH_KEY, String(value))
    } catch {
      // Persistence is optional.
    }
  }

  const runners = runnersQuery.data?.items ?? []
  const sessions = sessionsQuery.data?.items ?? []
  const runnerTotal = runnersQuery.data?.total ?? 0
  const sessionTotal = sessionsQuery.data?.total ?? 0
  const summary = summaryQuery.data

  const runnerColumns = useMemo<ColumnDef<WorkspaceRunnerResponse, unknown>[]>(
    () => [
      {
        accessorKey: 'container_name',
        header: t('operations.runners.table.runner'),
        cell: ({ row }) => (
          <div className="min-w-0">
            <div className="truncate font-mono text-sm">
              {row.original.container_name}
            </div>
            <div className="truncate text-xs text-muted-foreground">
              {row.original.image_name}
            </div>
          </div>
        ),
      },
      {
        accessorKey: 'owner_kind',
        header: t('operations.runners.table.owner'),
        cell: ({ row }) => (
          <span className="text-sm text-muted-foreground">
            {row.original.owner_kind}
            {row.original.owner_user_id ? (
              <span className="font-mono text-xs">
                #{row.original.owner_user_id}
              </span>
            ) : null}
          </span>
        ),
      },
      {
        accessorKey: 'status',
        header: t('operations.runners.table.status'),
        cell: ({ row }) => <StatusBadge status={row.original.status} />,
      },
      {
        accessorKey: 'last_active_at',
        header: t('operations.runners.table.lastActive'),
        cell: ({ row }) => (
          <span className="whitespace-nowrap text-sm text-muted-foreground">
            {formatDateTime(row.original.last_active_at)}
          </span>
        ),
      },
      {
        id: 'actions',
        header: '',
        cell: ({ row }) => (
          <div className="flex justify-end">
            <Button
              size="icon"
              variant="ghost"
              aria-label={t('operations.runners.detailsAriaLabel')}
              onClick={() => setRunnerDetail(row.original)}
            >
              <Eye aria-hidden="true" />
            </Button>
          </div>
        ),
      },
    ],
    [t],
  )

  const sessionColumns = useMemo<ColumnDef<RunnerSessionResponse, unknown>[]>(
    () => [
      {
        accessorKey: 'session_id',
        header: t('operations.sessions.table.session'),
        cell: ({ row }) => (
          <span className="font-mono text-sm">#{row.original.session_id}</span>
        ),
      },
      {
        accessorKey: 'owner_kind',
        header: t('operations.sessions.table.owner'),
        cell: ({ row }) => (
          <span className="text-sm text-muted-foreground">
            {row.original.owner_kind} #{row.original.owner_id}
          </span>
        ),
      },
      {
        accessorKey: 'state',
        header: t('operations.sessions.table.state'),
        cell: ({ row }) => <StatusBadge status={row.original.state} />,
      },
      {
        accessorKey: 'exit_code',
        header: t('operations.sessions.table.exit'),
        cell: ({ row }) => (
          <span
            className={
              row.original.exit_code === 0
                ? 'font-mono text-sm text-emerald-600 dark:text-emerald-400'
                : row.original.exit_code != null
                  ? 'font-mono text-sm text-destructive'
                  : 'font-mono text-sm text-muted-foreground'
            }
          >
            {row.original.exit_code ?? '—'}
          </span>
        ),
      },
      {
        accessorKey: 'wall_time_seconds',
        header: t('operations.sessions.table.duration'),
        cell: ({ row }) => (
          <span className="whitespace-nowrap text-sm text-muted-foreground">
            {formatDuration(row.original.wall_time_seconds)}
            {row.original.timed_out ? (
              <Badge variant="secondary" className="ml-1">
                {t('operations.timeout')}
              </Badge>
            ) : null}
          </span>
        ),
      },
      {
        id: 'actions',
        header: '',
        cell: ({ row }) => (
          <div className="flex justify-end">
            <Button
              size="icon"
              variant="ghost"
              aria-label={t('operations.sessions.detailsAriaLabel')}
              onClick={() => setSessionDetail(row.original)}
            >
              <Eye aria-hidden="true" />
            </Button>
          </div>
        ),
      },
    ],
    [t],
  )

  const error = runnersQuery.error ?? sessionsQuery.error ?? summaryQuery.error

  return (
    <div className="space-y-6">
      <PageHeader
        actions={
          <div className="flex items-center gap-2">
            <span className="flex items-center gap-1.5 text-sm text-muted-foreground">
              <Radar
                className={autoRefresh ? 'size-4 text-primary' : 'size-4'}
                aria-hidden="true"
              />
              <span className="hidden sm:inline">
                {t('operations.autoRefresh')}
              </span>
            </span>
            <Switch
              size="sm"
              checked={autoRefresh}
              aria-label={t('operations.autoRefresh')}
              onCheckedChange={toggleAutoRefresh}
            />
            <Button
              variant="outline"
              size="icon"
              disabled={runnersQuery.isFetching}
              aria-label={t('operations.actions.refresh')}
              onClick={refresh}
            >
              <RefreshCw aria-hidden="true" />
            </Button>
          </div>
        }
      />

      {error ? (
        <ErrorAlert
          error={errorMessage(error, t('operations.errors.load'))}
          onRetry={refresh}
        />
      ) : null}

      <div className="grid gap-4 sm:grid-cols-3">
        <StatCard
          title={t('operations.stats.activeSessions')}
          value={summary?.active_sessions ?? 0}
          icon={TerminalSquare}
          loading={summaryQuery.isPending}
        />
        <StatCard
          title={t('operations.stats.runningRunners')}
          value={summary?.active_runners ?? 0}
          icon={PlayCircle}
          loading={summaryQuery.isPending}
        />
        <StatCard
          title={t('operations.stats.failedOperations')}
          value={summary?.failed_operations ?? 0}
          icon={TriangleAlert}
          loading={summaryQuery.isPending}
        />
      </div>

      <Card>
        <Tabs
          value={activeTab}
          onValueChange={(value) =>
            setActiveTab(value as 'runners' | 'sessions')
          }
        >
          <div className="border-b px-4 pt-3">
            <TabsList>
              <TabsTrigger value="runners">
                {t('operations.tabs.runners', { count: runnerTotal })}
              </TabsTrigger>
              <TabsTrigger value="sessions">
                {t('operations.tabs.sessions', { count: sessionTotal })}
              </TabsTrigger>
            </TabsList>
          </div>

          <TabsContent value="runners">
            <CardContent className="p-0">
              <DataTable
                columns={runnerColumns}
                data={runners}
                getRowId={(row) => String(row.runner_id)}
                loading={runnersQuery.isPending}
                emptyTitle={t('operations.runners.emptyTitle')}
                emptyDescription={t('operations.runners.emptyDescription')}
                caption={t('operations.tabs.runners', { count: runnerTotal })}
              />
            </CardContent>
            <TablePagination
              page={runnersPage}
              totalPages={pageCount(runnerTotal, PAGE_SIZE)}
              onPageChange={setRunnersPage}
            />
          </TabsContent>

          <TabsContent value="sessions">
            <CardContent className="p-0">
              <DataTable
                columns={sessionColumns}
                data={sessions}
                getRowId={(row) => String(row.session_id)}
                loading={sessionsQuery.isPending}
                emptyTitle={t('operations.sessions.emptyTitle')}
                emptyDescription={t('operations.sessions.emptyDescription')}
                caption={t('operations.tabs.sessions', { count: sessionTotal })}
              />
            </CardContent>
            <TablePagination
              page={sessionsPage}
              totalPages={pageCount(sessionTotal, PAGE_SIZE)}
              onPageChange={setSessionsPage}
            />
          </TabsContent>
        </Tabs>
      </Card>

      <RunnerDetailDialog
        runner={runnerDetail}
        onClose={() => setRunnerDetail(null)}
      />
      <SessionDetailDialog
        session={sessionDetail}
        onClose={() => setSessionDetail(null)}
      />
    </div>
  )
}
