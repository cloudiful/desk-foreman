import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ColumnDef } from '@tanstack/react-table'
import { Archive, ArchiveRestore, GitBranch, RefreshCw } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  archiveWorkspaceBindingMutation,
  listWorkspaceBindingsOptions,
  listWorkspaceBindingsQueryKey,
  restoreWorkspaceBindingMutation,
  type ListWorkspaceBindingsData,
  type WorkspaceBindingResponse,
} from '@/api/generated'
import { errorMessage } from '@/api/http'
import { ConfirmDialog } from '@/components/confirm-dialog'
import { DataTable } from '@/components/data-table'
import { ErrorAlert } from '@/components/error-alert'
import { PageHeader } from '@/components/page-header'
import { StatusBadge } from '@/components/status-badge'
import { TablePagination } from '@/components/table-pagination'
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
import { formatDateTime } from '@/utils/format'
import { pageCount, pageOffset } from '@/utils/pagination'
import { WorkspaceGitUpstreamsDialog } from './workspace-git-upstreams-dialog'

type LifecycleFilter = 'all' | 'active' | 'archived'
type BindingAction = 'archive' | 'restore'

const BINDING_PAGE_SIZE = 20
const FILTER_DEBOUNCE_MS = 250

function truncateId(id: string | null | undefined): string {
  if (!id) return ''
  return id.length > 12 ? `${id.slice(0, 12)}…` : id
}

function parseApplicationId(value: string): number | undefined {
  if (!value.trim()) return undefined
  const parsed = Number(value)
  return Number.isInteger(parsed) && parsed > 0 ? parsed : undefined
}

export function WorkspaceBindingsPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [applicationId, setApplicationId] = useState('')
  const [externalUserId, setExternalUserId] = useState('')
  const [workspaceKey, setWorkspaceKey] = useState('')
  const [lifecycleFilter, setLifecycleFilter] = useState<LifecycleFilter>('all')
  const [page, setPage] = useState(1)
  const [upstreamsBinding, setUpstreamsBinding] =
    useState<WorkspaceBindingResponse | null>(null)
  const [actionTarget, setActionTarget] = useState<{
    binding: WorkspaceBindingResponse
    action: BindingAction
  } | null>(null)

  const debouncedApplicationId = useDebouncedValue(
    applicationId,
    FILTER_DEBOUNCE_MS,
  )
  const debouncedExternalUserId = useDebouncedValue(
    externalUserId,
    FILTER_DEBOUNCE_MS,
  )
  const debouncedWorkspaceKey = useDebouncedValue(
    workspaceKey,
    FILTER_DEBOUNCE_MS,
  )

  useEffect(() => {
    setPage(1)
  }, [
    debouncedApplicationId,
    debouncedExternalUserId,
    debouncedWorkspaceKey,
    lifecycleFilter,
  ])

  const query: NonNullable<ListWorkspaceBindingsData['query']> = {
    limit: BINDING_PAGE_SIZE,
    offset: pageOffset(page, BINDING_PAGE_SIZE),
    application_id: parseApplicationId(debouncedApplicationId),
    external_user_id: debouncedExternalUserId.trim() || undefined,
    workspace_key: debouncedWorkspaceKey.trim() || undefined,
    lifecycle_state: lifecycleFilter === 'all' ? undefined : lifecycleFilter,
  }

  const bindingsQuery = useQuery({
    ...listWorkspaceBindingsOptions({ query }),
    placeholderData: (previous) => previous,
  })

  const archiveBinding = useMutation({
    ...archiveWorkspaceBindingMutation(),
  })
  const restoreBinding = useMutation({
    ...restoreWorkspaceBindingMutation(),
  })

  const total = bindingsQuery.data?.total ?? 0
  const rows = bindingsQuery.data?.items ?? []
  const totalPages = pageCount(total, BINDING_PAGE_SIZE)

  useEffect(() => {
    if (bindingsQuery.isSuccess && page > totalPages) setPage(totalPages)
  }, [bindingsQuery.isSuccess, page, totalPages])

  function actionLabel(action: BindingAction): string {
    return action === 'archive'
      ? t('workspaceBindings.actions.archive')
      : t('workspaceBindings.actions.restore')
  }

  async function confirmAction(): Promise<void> {
    if (!actionTarget) return
    const mutation =
      actionTarget.action === 'archive' ? archiveBinding : restoreBinding
    try {
      await mutation.mutateAsync({
        path: { binding_id: actionTarget.binding.workspace_binding_id },
      })
      toast.success(
        actionTarget.action === 'archive'
          ? t('workspaceBindings.notifications.archiveSuccess')
          : t('workspaceBindings.notifications.restoreSuccess'),
        {
          description: t('workspaceBindings.notifications.binding', {
            id: actionTarget.binding.workspace_binding_id,
          }),
        },
      )
      setActionTarget(null)
      await queryClient.invalidateQueries({
        queryKey: listWorkspaceBindingsQueryKey(),
      })
    } catch (error) {
      toast.error(
        actionTarget.action === 'archive'
          ? t('workspaceBindings.errors.archive')
          : t('workspaceBindings.errors.restore'),
        {
          description: errorMessage(
            error,
            actionTarget.action === 'archive'
              ? t('workspaceBindings.errors.archive')
              : t('workspaceBindings.errors.restore'),
          ),
        },
      )
    }
  }

  const columns: ColumnDef<WorkspaceBindingResponse, unknown>[] = [
    {
      accessorKey: 'workspace_binding_id',
      header: t('workspaceBindings.table.binding'),
      cell: ({ row }) => (
        <span className="font-mono text-sm">
          #{row.original.workspace_binding_id}
        </span>
      ),
    },
    {
      id: 'identity',
      header: t('workspaceBindings.table.identity'),
      cell: ({ row }) => (
        <div className="min-w-0">
          <div className="text-sm">
            {t('workspaceBindings.identity.application')} #
            {row.original.application_id}
          </div>
          <div className="truncate text-xs text-muted-foreground">
            {row.original.external_user_id}
            {row.original.resource_kind ? (
              <span>
                {' · '}
                {row.original.resource_kind}:
                {truncateId(row.original.resource_id)}
              </span>
            ) : null}
          </div>
        </div>
      ),
    },
    {
      id: 'workspace',
      header: t('workspaceBindings.table.workspace'),
      cell: ({ row }) => (
        <div className="min-w-0">
          <div className="truncate font-mono text-xs text-muted-foreground">
            {row.original.workspace_root}
          </div>
          <div className="text-xs text-muted-foreground">
            {row.original.workspace_key}
          </div>
        </div>
      ),
    },
    {
      accessorKey: 'lifecycle_state',
      header: t('workspaceBindings.table.state'),
      cell: ({ row }) => <StatusBadge status={row.original.lifecycle_state} />,
    },
    {
      accessorKey: 'last_used_at',
      header: t('workspaceBindings.table.lastUsed'),
      cell: ({ row }) => (
        <span className="whitespace-nowrap text-sm text-muted-foreground">
          {formatDateTime(row.original.last_used_at)}
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
            aria-label={t('workspaceBindings.upstreams.actions.open')}
            onClick={() => setUpstreamsBinding(row.original)}
          >
            <GitBranch aria-hidden="true" />
          </Button>
          {row.original.lifecycle_state === 'active' ? (
            <Button
              size="icon"
              variant="ghost"
              aria-label={t('workspaceBindings.actions.archive')}
              onClick={() =>
                setActionTarget({ binding: row.original, action: 'archive' })
              }
            >
              <Archive aria-hidden="true" />
            </Button>
          ) : (
            <Button
              size="icon"
              variant="ghost"
              aria-label={t('workspaceBindings.actions.restore')}
              onClick={() =>
                setActionTarget({ binding: row.original, action: 'restore' })
              }
            >
              <ArchiveRestore aria-hidden="true" />
            </Button>
          )}
        </div>
      ),
    },
  ]

  const pending = archiveBinding.isPending || restoreBinding.isPending

  return (
    <div className="space-y-6">
      <PageHeader
        actions={
          <Button
            variant="outline"
            size="icon"
            disabled={bindingsQuery.isFetching}
            aria-label={t('workspaceBindings.actions.refresh')}
            onClick={() => void bindingsQuery.refetch()}
          >
            <RefreshCw aria-hidden="true" />
          </Button>
        }
      />

      <Card>
        <div className="flex flex-col flex-wrap gap-3 border-b p-4 md:flex-row md:items-center">
          <Input
            value={applicationId}
            onChange={(event) => setApplicationId(event.target.value)}
            placeholder={t('workspaceBindings.filters.applicationId')}
            aria-label={t('workspaceBindings.filters.applicationId')}
            type="number"
            className="w-32"
          />
          <Input
            value={externalUserId}
            onChange={(event) => setExternalUserId(event.target.value)}
            placeholder={t('workspaceBindings.filters.externalUserId')}
            aria-label={t('workspaceBindings.filters.externalUserId')}
            className="md:max-w-xs"
          />
          <Input
            value={workspaceKey}
            onChange={(event) => setWorkspaceKey(event.target.value)}
            placeholder={t('workspaceBindings.filters.workspaceKey')}
            aria-label={t('workspaceBindings.filters.workspaceKey')}
            className="md:max-w-xs"
          />
          <Select
            value={lifecycleFilter}
            onValueChange={(value) =>
              setLifecycleFilter(value as LifecycleFilter)
            }
          >
            <SelectTrigger
              className="w-36"
              aria-label={t('workspaceBindings.filters.allStates')}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">
                {t('workspaceBindings.filters.allStates')}
              </SelectItem>
              <SelectItem value="active">
                {t('workspaceBindings.filters.active')}
              </SelectItem>
              <SelectItem value="archived">
                {t('workspaceBindings.filters.archived')}
              </SelectItem>
            </SelectContent>
          </Select>
          <Button
            variant="secondary"
            disabled={bindingsQuery.isFetching}
            onClick={() => {
              setPage(1)
              void bindingsQuery.refetch()
            }}
          >
            {t('workspaceBindings.actions.apply')}
          </Button>
        </div>

        <CardContent className="p-0">
          {bindingsQuery.error ? (
            <div className="p-4">
              <ErrorAlert
                error={errorMessage(
                  bindingsQuery.error,
                  t('workspaceBindings.errors.load'),
                )}
                onRetry={() => void bindingsQuery.refetch()}
              />
            </div>
          ) : (
            <DataTable
              columns={columns}
              data={rows}
              getRowId={(row) => String(row.workspace_binding_id)}
              loading={bindingsQuery.isPending}
              emptyTitle={t('workspaceBindings.empty.title')}
              emptyDescription={t('workspaceBindings.empty.description')}
              caption={t('workspaceBindings.table.binding')}
            />
          )}
        </CardContent>

        <TablePagination
          page={page}
          totalPages={totalPages}
          onPageChange={setPage}
        />
      </Card>

      {upstreamsBinding ? (
        <WorkspaceGitUpstreamsDialog
          key={upstreamsBinding.workspace_binding_id}
          binding={upstreamsBinding}
          onOpenChange={(open) => {
            if (!open) setUpstreamsBinding(null)
          }}
        />
      ) : null}

      <ConfirmDialog
        open={actionTarget !== null}
        title={
          actionTarget
            ? t('workspaceBindings.confirmations.title', {
                action: actionLabel(actionTarget.action),
              })
            : ''
        }
        description={
          actionTarget
            ? t(
                actionTarget.action === 'archive'
                  ? 'workspaceBindings.confirmations.archiveDescription'
                  : 'workspaceBindings.confirmations.restoreDescription',
                {
                  id: `#${actionTarget.binding.workspace_binding_id}`,
                },
              )
            : undefined
        }
        confirmLabel={
          actionTarget
            ? actionLabel(actionTarget.action)
            : t('shared.confirm.confirm')
        }
        cancelLabel={t('shared.confirm.cancel')}
        destructive={actionTarget?.action === 'archive'}
        pending={pending}
        onConfirm={() => void confirmAction()}
        onOpenChange={(open) => {
          if (!open && !pending) setActionTarget(null)
        }}
      />
    </div>
  )
}
