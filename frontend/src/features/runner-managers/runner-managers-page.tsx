import { useQuery } from '@tanstack/react-query'
import type { ColumnDef } from '@tanstack/react-table'
import {
  CircleAlert,
  CircleCheck,
  CircleOff,
  Pencil,
  Plus,
  RefreshCw,
  ServerCog,
} from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  listRunnerManagersOptions,
  type RunnerManagerResponse,
} from '@/api/generated'
import { errorMessage } from '@/api/http'
import { DataTable } from '@/components/data-table'
import { ErrorAlert } from '@/components/error-alert'
import { PageHeader } from '@/components/page-header'
import { StatCard } from '@/components/stat-card'
import { StatusBadge } from '@/components/status-badge'
import { TablePagination } from '@/components/table-pagination'
import { TokenReveal } from '@/components/token-reveal'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { useDebouncedValue } from '@/hooks/use-debounced-value'
import { formatBytes, formatMilliseconds, formatRelative } from '@/utils/format'
import { PAGE_SIZE, pageCount, pageOffset } from '@/utils/pagination'
import { RunnerManagerFormDialog } from './runner-manager-form-dialog'

const FILTER_DEBOUNCE_MS = 250

export function RunnerManagersPage() {
  const { t } = useTranslation()
  const [search, setSearch] = useState('')
  const [page, setPage] = useState(1)
  const [formTarget, setFormTarget] = useState<
    RunnerManagerResponse | null | undefined
  >(undefined)
  const [createdToken, setCreatedToken] = useState<string | null>(null)

  const debouncedSearch = useDebouncedValue(search, FILTER_DEBOUNCE_MS)

  useEffect(() => {
    setPage(1)
  }, [debouncedSearch])

  const managersQuery = useQuery({
    ...listRunnerManagersOptions({
      query: {
        limit: PAGE_SIZE,
        offset: pageOffset(page, PAGE_SIZE),
        search: debouncedSearch.trim() || undefined,
      },
    }),
    placeholderData: (previous) => previous,
  })

  const total = managersQuery.data?.total ?? 0
  const rows = managersQuery.data?.items ?? []
  const totalPages = pageCount(total, PAGE_SIZE)
  const stats = {
    online: rows.filter(
      (manager) => manager.enabled && manager.status === 'online',
    ).length,
    offline: rows.filter(
      (manager) => manager.enabled && manager.status !== 'online',
    ).length,
    disabled: rows.filter((manager) => !manager.enabled).length,
  }

  useEffect(() => {
    if (managersQuery.isSuccess && page > totalPages) setPage(totalPages)
  }, [managersQuery.isSuccess, page, totalPages])

  const columns: ColumnDef<RunnerManagerResponse, unknown>[] = [
    {
      accessorKey: 'name',
      header: t('runnerManagers.table.manager'),
      cell: ({ row }) => (
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <ServerCog
              className="size-4 shrink-0 text-muted-foreground"
              aria-hidden="true"
            />
            <span className="truncate font-medium">{row.original.name}</span>
          </div>
          <div className="mt-0.5 truncate font-mono text-xs text-muted-foreground">
            {row.original.endpoint}
          </div>
          <div className="truncate text-xs text-muted-foreground">
            {row.original.image}
          </div>
        </div>
      ),
    },
    {
      id: 'status',
      header: t('runnerManagers.table.status'),
      cell: ({ row }) => (
        <StatusBadge
          status={
            !row.original.enabled
              ? 'disabled'
              : row.original.status === 'online'
                ? 'online'
                : 'offline'
          }
        />
      ),
    },
    {
      id: 'limits',
      header: t('runnerManagers.table.limits'),
      cell: ({ row }) => (
        <span className="text-xs text-muted-foreground">
          {t('runnerManagers.limits', {
            sessions: row.original.max_sessions,
            timeout: formatMilliseconds(row.original.max_timeout_ms),
            output: formatBytes(row.original.max_output_bytes),
          })}
        </span>
      ),
    },
    {
      accessorKey: 'last_seen_at',
      header: t('runnerManagers.table.lastSeen'),
      cell: ({ row }) => (
        <span className="whitespace-nowrap text-sm text-muted-foreground">
          {formatRelative(row.original.last_seen_at)}
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
            aria-label={t('runnerManagers.actions.edit')}
            onClick={() => setFormTarget(row.original)}
          >
            <Pencil aria-hidden="true" />
          </Button>
        </div>
      ),
    },
  ]

  return (
    <div className="space-y-6">
      <PageHeader
        actions={
          <>
            <Button
              variant="outline"
              size="icon"
              disabled={managersQuery.isFetching}
              aria-label={t('runnerManagers.actions.refresh')}
              onClick={() => void managersQuery.refetch()}
            >
              <RefreshCw aria-hidden="true" />
            </Button>
            <Button
              aria-label={t('runnerManagers.actions.add')}
              onClick={() => setFormTarget(null)}
            >
              <Plus aria-hidden="true" />
              <span className="hidden sm:inline">
                {t('runnerManagers.actions.add')}
              </span>
            </Button>
          </>
        }
      />

      <div className="grid gap-4 sm:grid-cols-3">
        <StatCard
          title={t('runnerManagers.stats.online')}
          value={stats.online}
          icon={CircleCheck}
          loading={managersQuery.isPending}
        />
        <StatCard
          title={t('runnerManagers.stats.offline')}
          value={stats.offline}
          icon={CircleAlert}
          loading={managersQuery.isPending}
        />
        <StatCard
          title={t('runnerManagers.stats.disabled')}
          value={stats.disabled}
          icon={CircleOff}
          loading={managersQuery.isPending}
        />
      </div>

      <Card>
        <div className="border-b p-4">
          <Input
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder={t('runnerManagers.searchPlaceholder')}
            aria-label={t('runnerManagers.searchPlaceholder')}
            className="md:max-w-xs"
          />
        </div>

        <CardContent className="p-0">
          {managersQuery.error ? (
            <div className="p-4">
              <ErrorAlert
                error={errorMessage(
                  managersQuery.error,
                  t('runnerManagers.errors.load'),
                )}
                onRetry={() => void managersQuery.refetch()}
              />
            </div>
          ) : (
            <DataTable
              columns={columns}
              data={rows}
              getRowId={(row) => String(row.runner_manager_id)}
              loading={managersQuery.isPending}
              emptyTitle={
                search
                  ? t('runnerManagers.empty.matchingTitle')
                  : t('runnerManagers.empty.title')
              }
              emptyDescription={
                search
                  ? t('runnerManagers.empty.searchDescription')
                  : t('runnerManagers.empty.description')
              }
              caption={t('runnerManagers.table.manager')}
            />
          )}
        </CardContent>

        <TablePagination
          page={page}
          totalPages={totalPages}
          onPageChange={setPage}
        />
      </Card>

      {formTarget !== undefined ? (
        <RunnerManagerFormDialog
          key={formTarget?.runner_manager_id ?? 'new'}
          manager={formTarget}
          onCreatedToken={setCreatedToken}
          onOpenChange={(open) => {
            if (!open) setFormTarget(undefined)
          }}
        />
      ) : null}

      <Dialog
        open={createdToken !== null}
        onOpenChange={(open) => {
          if (!open) setCreatedToken(null)
        }}
      >
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>{t('runnerManagers.token.title')}</DialogTitle>
            <DialogDescription>
              {t('runnerManagers.token.description')}
            </DialogDescription>
          </DialogHeader>
          {createdToken ? (
            <TokenReveal
              token={createdToken}
              onDismiss={() => setCreatedToken(null)}
            />
          ) : null}
          <p className="text-xs text-muted-foreground">
            {t('runnerManagers.token.configure')}{' '}
            <code className="font-mono">RUNNER_MANAGER_TOKEN</code>.
          </p>
          <DialogFooter>
            <Button type="button" onClick={() => setCreatedToken(null)}>
              {t('runnerManagers.actions.done')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
