import { useQuery } from '@tanstack/react-query'
import type { ColumnDef } from '@tanstack/react-table'
import { AppWindow, KeyRound, Pencil, Plus, RefreshCw } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  listApplicationsOptions,
  type ApplicationResponse,
} from '@/api/generated'
import { errorMessage } from '@/api/http'
import { DataTable } from '@/components/data-table'
import { ErrorAlert } from '@/components/error-alert'
import { PageHeader } from '@/components/page-header'
import { StatusBadge } from '@/components/status-badge'
import { TablePagination } from '@/components/table-pagination'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { useDebouncedValue } from '@/hooks/use-debounced-value'
import { formatBytes, formatMilliseconds, formatRelative } from '@/utils/format'
import { PAGE_SIZE, pageCount, pageOffset } from '@/utils/pagination'
import { ApplicationFormDialog } from './application-form-dialog'
import { ApplicationTokensDialog } from './application-tokens-dialog'

type StatusFilter = 'all' | 'active' | 'inactive'

const FILTER_DEBOUNCE_MS = 250

export function ApplicationsPage() {
  const { t } = useTranslation()
  const [search, setSearch] = useState('')
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('all')
  const [page, setPage] = useState(1)
  const [formTarget, setFormTarget] = useState<
    ApplicationResponse | null | undefined
  >(undefined)
  const [tokensTarget, setTokensTarget] = useState<ApplicationResponse | null>(
    null,
  )

  const debouncedSearch = useDebouncedValue(search, FILTER_DEBOUNCE_MS)

  useEffect(() => {
    setPage(1)
  }, [debouncedSearch, statusFilter])

  const applicationsQuery = useQuery({
    ...listApplicationsOptions({
      query: {
        limit: PAGE_SIZE,
        offset: pageOffset(page, PAGE_SIZE),
        search: debouncedSearch.trim() || undefined,
        is_active:
          statusFilter === 'all' ? undefined : statusFilter === 'active',
      },
    }),
    placeholderData: (previous) => previous,
  })

  const total = applicationsQuery.data?.total ?? 0
  const rows = applicationsQuery.data?.items ?? []
  const totalPages = pageCount(total, PAGE_SIZE)

  useEffect(() => {
    if (applicationsQuery.isSuccess && page > totalPages) setPage(totalPages)
  }, [applicationsQuery.isSuccess, page, totalPages])

  const columns: ColumnDef<ApplicationResponse, unknown>[] = [
    {
      accessorKey: 'name',
      header: t('applications.table.application'),
      cell: ({ row }) => (
        <div className="flex items-center gap-2">
          <div className="flex size-8 items-center justify-center rounded-md bg-muted text-muted-foreground">
            <AppWindow className="size-4" aria-hidden="true" />
          </div>
          <div className="min-w-0">
            <div className="truncate font-medium">{row.original.name}</div>
            <div className="truncate font-mono text-xs text-muted-foreground">
              #{row.original.application_id} ·{' '}
              {row.original.default_shell ??
                t('applications.limits.defaultShell')}
            </div>
          </div>
        </div>
      ),
    },
    {
      id: 'limits',
      header: t('applications.table.limits'),
      cell: ({ row }) => (
        <span className="text-xs text-muted-foreground">
          {row.original.max_timeout_ms
            ? t('applications.limits.timeout', {
                value: formatMilliseconds(row.original.max_timeout_ms),
              })
            : t('applications.limits.noTimeout')}
          <br />
          {row.original.max_output_bytes
            ? t('applications.limits.output', {
                value: formatBytes(row.original.max_output_bytes),
              })
            : t('applications.limits.unlimitedOutput')}
        </span>
      ),
    },
    {
      id: 'scopes',
      header: t('applications.table.scopes'),
      cell: ({ row }) => (
        <Badge variant="secondary">
          {t('applications.limits.scopes', {
            count: row.original.default_scopes.length,
          })}
        </Badge>
      ),
    },
    {
      id: 'status',
      header: t('applications.table.status'),
      cell: ({ row }) => (
        <StatusBadge status={row.original.is_active ? 'active' : 'inactive'} />
      ),
    },
    {
      accessorKey: 'updated_at',
      header: t('applications.table.updated'),
      cell: ({ row }) => (
        <span className="whitespace-nowrap text-sm text-muted-foreground">
          {formatRelative(row.original.updated_at)}
        </span>
      ),
    },
    {
      id: 'actions',
      header: '',
      cell: ({ row }) => (
        <div className="flex justify-end gap-1">
          <Button
            size="icon"
            variant="ghost"
            aria-label={t('applications.actions.edit')}
            onClick={() => setFormTarget(row.original)}
          >
            <Pencil aria-hidden="true" />
          </Button>
          <Button
            size="icon"
            variant="ghost"
            aria-label={t('applications.actions.tokens')}
            onClick={() => setTokensTarget(row.original)}
          >
            <KeyRound aria-hidden="true" />
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
              disabled={applicationsQuery.isFetching}
              aria-label={t('applications.actions.refresh')}
              onClick={() => void applicationsQuery.refetch()}
            >
              <RefreshCw aria-hidden="true" />
            </Button>
            <Button onClick={() => setFormTarget(null)}>
              <Plus aria-hidden="true" />
              {t('applications.actions.create')}
            </Button>
          </>
        }
      />

      <Card>
        <div className="flex flex-col gap-3 border-b p-4 md:flex-row md:items-center">
          <Input
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder={t('applications.searchPlaceholder')}
            aria-label={t('applications.searchPlaceholder')}
            className="md:max-w-xs"
          />
          <Select
            value={statusFilter}
            onValueChange={(value) => setStatusFilter(value as StatusFilter)}
          >
            <SelectTrigger
              className="w-36"
              aria-label={t('applications.filters.allStatuses')}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">
                {t('applications.filters.allStatuses')}
              </SelectItem>
              <SelectItem value="active">
                {t('applications.filters.active')}
              </SelectItem>
              <SelectItem value="inactive">
                {t('applications.filters.inactive')}
              </SelectItem>
            </SelectContent>
          </Select>
          <span className="ml-auto text-sm text-muted-foreground">
            {t('applications.count', { total })}
          </span>
        </div>

        <CardContent className="p-0">
          {applicationsQuery.error ? (
            <div className="p-4">
              <ErrorAlert
                error={errorMessage(
                  applicationsQuery.error,
                  t('applications.errors.load'),
                )}
                onRetry={() => void applicationsQuery.refetch()}
              />
            </div>
          ) : (
            <DataTable
              columns={columns}
              data={rows}
              getRowId={(row) => String(row.application_id)}
              loading={applicationsQuery.isPending}
              emptyTitle={
                search
                  ? t('applications.empty.matchingTitle')
                  : t('applications.empty.title')
              }
              emptyDescription={
                search
                  ? t('applications.empty.searchDescription')
                  : t('applications.empty.description')
              }
              caption={t('applications.table.application')}
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
        <ApplicationFormDialog
          key={formTarget?.application_id ?? 'new'}
          application={formTarget}
          onOpenChange={(open) => {
            if (!open) setFormTarget(undefined)
          }}
        />
      ) : null}

      {tokensTarget ? (
        <ApplicationTokensDialog
          key={tokensTarget.application_id}
          application={tokensTarget}
          onOpenChange={(open) => {
            if (!open) setTokensTarget(null)
          }}
        />
      ) : null}
    </div>
  )
}
