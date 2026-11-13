import { useQuery } from '@tanstack/react-query'
import type { ColumnDef } from '@tanstack/react-table'
import { Eye, RefreshCw } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { listAuditLogsOptions, type AuditLogResponse } from '@/api/generated'
import { errorMessage } from '@/api/http'
import { DataTable } from '@/components/data-table'
import { ErrorAlert } from '@/components/error-alert'
import { PageHeader } from '@/components/page-header'
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
import { AUDIT_PAGE_SIZE, pageCount, pageOffset } from '@/utils/pagination'
import { formatDateTime } from '@/utils/format'
import { AuditDetailDialog } from './audit-detail-dialog'

type OutcomeFilter = 'all' | 'success' | 'failure' | 'unknown'

const FILTER_DEBOUNCE_MS = 250

function truncateId(id: string): string {
  if (!id) return ''
  return id.length > 14 ? `${id.slice(0, 14)}…` : id
}

export function AuditPage() {
  const { t } = useTranslation()
  const [actionFilter, setActionFilter] = useState('')
  const [statusFilter, setStatusFilter] = useState<OutcomeFilter>('all')
  const [page, setPage] = useState(1)
  const [detail, setDetail] = useState<AuditLogResponse | null>(null)

  const debouncedActionFilter = useDebouncedValue(
    actionFilter,
    FILTER_DEBOUNCE_MS,
  )

  useEffect(() => {
    setPage(1)
  }, [actionFilter, statusFilter])

  const auditQuery = useQuery({
    ...listAuditLogsOptions({
      query: {
        limit: AUDIT_PAGE_SIZE,
        offset: pageOffset(page, AUDIT_PAGE_SIZE),
        search: debouncedActionFilter.trim() || undefined,
        status: statusFilter === 'all' ? undefined : statusFilter,
      },
    }),
    placeholderData: (previous) => previous,
  })

  const total = auditQuery.data?.total ?? 0
  const rows = auditQuery.data?.items ?? []
  const totalPages = pageCount(total, AUDIT_PAGE_SIZE)

  useEffect(() => {
    if (auditQuery.isSuccess && page > totalPages) setPage(totalPages)
  }, [auditQuery.isSuccess, page, totalPages])

  const columns = useMemo<ColumnDef<AuditLogResponse, unknown>[]>(
    () => [
      {
        accessorKey: 'created_at',
        header: t('audit.columns.time'),
        cell: ({ row }) => (
          <span className="whitespace-nowrap text-sm text-muted-foreground">
            {formatDateTime(row.original.created_at)}
          </span>
        ),
      },
      {
        accessorKey: 'action',
        header: t('audit.columns.action'),
        cell: ({ row }) => (
          <Badge variant="secondary">
            <code className="font-mono">{row.original.action}</code>
          </Badge>
        ),
      },
      {
        accessorKey: 'actor_type',
        header: t('audit.columns.actor'),
        cell: ({ row }) => (
          <span className="text-sm text-muted-foreground">
            {row.original.actor_type}
            {row.original.actor_user_id ? (
              <span className="font-mono text-xs">
                #{row.original.actor_user_id}
              </span>
            ) : null}
            {row.original.actor_application_id ? (
              <span className="font-mono text-xs">
                #{row.original.actor_application_id}
              </span>
            ) : null}
          </span>
        ),
      },
      {
        accessorKey: 'target_type',
        header: t('audit.columns.target'),
        cell: ({ row }) => (
          <span className="text-sm text-muted-foreground">
            {row.original.target_type}
            <span className="font-mono text-xs">
              #{truncateId(row.original.target_id)}
            </span>
          </span>
        ),
      },
      {
        accessorKey: 'status',
        header: t('audit.columns.outcome'),
        cell: ({ row }) => (
          <span className="text-sm">
            {row.original.status
              ? t(`audit.status.${row.original.status}`, {
                  defaultValue: row.original.status,
                })
              : t('audit.notAvailable')}
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
              aria-label={t('audit.detailsAriaLabel')}
              onClick={() => setDetail(row.original)}
            >
              <Eye aria-hidden="true" />
            </Button>
          </div>
        ),
      },
    ],
    [t],
  )

  return (
    <div className="space-y-6">
      <PageHeader
        actions={
          <Button
            variant="outline"
            disabled={auditQuery.isFetching}
            aria-label={t('audit.refresh')}
            onClick={() => void auditQuery.refetch()}
          >
            <RefreshCw aria-hidden="true" />
          </Button>
        }
      />

      <Card>
        <div className="flex flex-col gap-3 border-b p-4 md:flex-row md:items-center">
          <Input
            value={actionFilter}
            onChange={(event) => setActionFilter(event.target.value)}
            placeholder={t('audit.filterPlaceholder')}
            aria-label={t('audit.filterPlaceholder')}
            className="md:max-w-sm"
          />
          <Select
            value={statusFilter}
            onValueChange={(value) => setStatusFilter(value as OutcomeFilter)}
          >
            <SelectTrigger
              className="w-36"
              aria-label={t('audit.outcomes.all')}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">{t('audit.outcomes.all')}</SelectItem>
              <SelectItem value="success">
                {t('audit.outcomes.success')}
              </SelectItem>
              <SelectItem value="failure">
                {t('audit.outcomes.failure')}
              </SelectItem>
              <SelectItem value="unknown">
                {t('audit.outcomes.unknown')}
              </SelectItem>
            </SelectContent>
          </Select>
          <span className="ml-auto text-sm text-muted-foreground">
            {t('audit.eventsCount', { shown: rows.length, total })}
          </span>
        </div>

        <CardContent className="p-0">
          {auditQuery.error ? (
            <div className="p-4">
              <ErrorAlert
                error={errorMessage(auditQuery.error, t('audit.errors.load'))}
                onRetry={() => void auditQuery.refetch()}
              />
            </div>
          ) : (
            <DataTable
              columns={columns}
              data={rows}
              getRowId={(row) => String(row.audit_id)}
              loading={auditQuery.isPending}
              emptyTitle={t('audit.noEvents')}
              emptyDescription={t('audit.noEventsDescription')}
              caption={t('audit.detailTitle')}
            />
          )}
        </CardContent>

        <TablePagination
          page={page}
          totalPages={totalPages}
          onPageChange={setPage}
        />
      </Card>

      <AuditDetailDialog event={detail} onClose={() => setDetail(null)} />
    </div>
  )
}
