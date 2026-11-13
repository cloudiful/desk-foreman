import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ColumnDef } from '@tanstack/react-table'
import {
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  KeyRound,
  MoreHorizontal,
  Pencil,
  Plus,
  RefreshCw,
} from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  deleteUserMutation,
  listUsersOptions,
  listUsersQueryKey,
  updateUserMutation,
  type ListUsersData,
  type UserResponse,
} from '@/api/generated'
import { errorMessage } from '@/api/http'
import { ConfirmDialog } from '@/components/confirm-dialog'
import { DataTable } from '@/components/data-table'
import { ErrorAlert } from '@/components/error-alert'
import { PageHeader } from '@/components/page-header'
import { StatusBadge } from '@/components/status-badge'
import { TablePagination } from '@/components/table-pagination'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { useDebouncedValue } from '@/hooks/use-debounced-value'
import { formatDateTime, formatRelative, initials } from '@/utils/format'
import { PAGE_SIZE, pageCount, pageOffset } from '@/utils/pagination'
import { ResetPasswordDialog } from './reset-password-dialog'
import { UserFormDialog } from './user-form-dialog'
import { UserTokensDialog } from './user-tokens-dialog'

type RoleFilter = 'all' | 'admin' | 'user'
type StatusFilter = 'all' | 'active' | 'inactive'
type SortKey = 'login_name' | 'last_login_at' | 'updated_at'

const FILTER_DEBOUNCE_MS = 250

interface SortState {
  id: SortKey
  desc: boolean
}

export function UsersPage() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [search, setSearch] = useState('')
  const [roleFilter, setRoleFilter] = useState<RoleFilter>('all')
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('all')
  const [page, setPage] = useState(1)
  const [sort, setSort] = useState<SortState>({ id: 'updated_at', desc: true })
  const [formTarget, setFormTarget] = useState<UserResponse | null | undefined>(
    undefined,
  )
  const [resetTarget, setResetTarget] = useState<UserResponse | null>(null)
  const [tokensTarget, setTokensTarget] = useState<UserResponse | null>(null)
  const [deactivateTarget, setDeactivateTarget] = useState<UserResponse | null>(
    null,
  )

  const debouncedSearch = useDebouncedValue(search, FILTER_DEBOUNCE_MS)

  useEffect(() => {
    setPage(1)
  }, [debouncedSearch, roleFilter, statusFilter])

  const query: NonNullable<ListUsersData['query']> = {
    limit: PAGE_SIZE,
    offset: pageOffset(page, PAGE_SIZE),
    sort_by: sort.id,
    sort_dir: sort.desc ? 'desc' : 'asc',
    search: debouncedSearch.trim() || undefined,
    is_admin: roleFilter === 'all' ? undefined : roleFilter === 'admin',
    is_active: statusFilter === 'all' ? undefined : statusFilter === 'active',
  }

  const usersQuery = useQuery({
    ...listUsersOptions({ query }),
    placeholderData: (previous) => previous,
  })

  const activateUser = useMutation({ ...updateUserMutation() })
  const deactivateUser = useMutation({ ...deleteUserMutation() })

  const total = usersQuery.data?.total ?? 0
  const rows = usersQuery.data?.items ?? []
  const totalPages = pageCount(total, PAGE_SIZE)

  useEffect(() => {
    if (usersQuery.isSuccess && page > totalPages) setPage(totalPages)
  }, [usersQuery.isSuccess, page, totalPages])

  function changeSort(id: SortKey): void {
    setSort((current) =>
      current.id === id
        ? { id, desc: !current.desc }
        : { id, desc: id !== 'login_name' },
    )
    setPage(1)
  }

  function sortableHeader(label: string, id: SortKey) {
    const active = sort.id === id
    const Icon = active ? (sort.desc ? ArrowDown : ArrowUp) : ArrowUpDown
    return (
      <Button
        variant="ghost"
        size="sm"
        className="-ml-2 h-7 gap-1 px-2"
        aria-label={t('shared.sortBy', { column: label })}
        onClick={() => changeSort(id)}
      >
        {label}
        <Icon className="size-3.5" aria-hidden="true" />
      </Button>
    )
  }

  async function activate(row: UserResponse): Promise<void> {
    try {
      await activateUser.mutateAsync({
        path: { user_id: row.user_id },
        body: {
          display_name: row.display_name,
          email: row.email,
          timezone: row.timezone,
          is_admin: row.is_admin,
          is_active: true,
        },
      })
      toast.success(t('users.notifications.userActivated'), {
        description: row.login_name,
      })
      await queryClient.invalidateQueries({ queryKey: listUsersQueryKey() })
    } catch (error) {
      toast.error(t('users.errors.activate'), {
        description: errorMessage(error, t('users.errors.activate')),
      })
    }
  }

  async function confirmDeactivate(): Promise<void> {
    if (!deactivateTarget) return
    try {
      await deactivateUser.mutateAsync({
        path: { user_id: deactivateTarget.user_id },
      })
      toast.success(t('users.notifications.userDeactivated'), {
        description: deactivateTarget.login_name,
      })
      setDeactivateTarget(null)
      await queryClient.invalidateQueries({ queryKey: listUsersQueryKey() })
    } catch (error) {
      toast.error(t('users.errors.deactivate'), {
        description: errorMessage(error, t('users.errors.deactivate')),
      })
    }
  }

  const columns: ColumnDef<UserResponse, unknown>[] = [
    {
      accessorKey: 'login_name',
      header: () => sortableHeader(t('users.table.user'), 'login_name'),
      cell: ({ row }) => (
        <div className="flex items-center gap-3">
          <div className="flex size-8 shrink-0 items-center justify-center rounded-full bg-secondary text-xs font-medium text-secondary-foreground">
            {initials(row.original.display_name || row.original.login_name)}
          </div>
          <div className="min-w-0">
            <div className="truncate font-medium">
              {row.original.display_name}
            </div>
            <div className="truncate text-xs text-muted-foreground">
              @{row.original.login_name}
            </div>
          </div>
        </div>
      ),
    },
    {
      id: 'role',
      header: t('users.table.role'),
      cell: ({ row }) =>
        row.original.is_admin ? (
          <Badge>{t('users.roles.admin')}</Badge>
        ) : (
          <span className="text-sm text-muted-foreground">
            {t('users.roles.user')}
          </span>
        ),
    },
    {
      id: 'status',
      header: t('users.table.status'),
      cell: ({ row }) => (
        <StatusBadge status={row.original.is_active ? 'active' : 'inactive'} />
      ),
    },
    {
      accessorKey: 'last_login_at',
      header: () => sortableHeader(t('users.table.lastLogin'), 'last_login_at'),
      cell: ({ row }) => (
        <span
          className="whitespace-nowrap text-sm"
          title={formatDateTime(row.original.last_login_at)}
        >
          {formatRelative(row.original.last_login_at)}
        </span>
      ),
    },
    {
      accessorKey: 'updated_at',
      header: () => sortableHeader(t('users.table.updated'), 'updated_at'),
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
            aria-label={t('users.actions.edit')}
            onClick={() => setFormTarget(row.original)}
          >
            <Pencil aria-hidden="true" />
          </Button>
          <Button
            size="icon"
            variant="ghost"
            aria-label={t('users.actions.tokens')}
            onClick={() => setTokensTarget(row.original)}
          >
            <KeyRound aria-hidden="true" />
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                size="icon"
                variant="ghost"
                aria-label={t('users.actions.more')}
              >
                <MoreHorizontal aria-hidden="true" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={() => setResetTarget(row.original)}>
                <RefreshCw aria-hidden="true" />
                {t('users.actions.resetPassword')}
              </DropdownMenuItem>
              {row.original.is_active ? (
                <DropdownMenuItem
                  className="text-destructive focus:text-destructive"
                  onSelect={() => setDeactivateTarget(row.original)}
                >
                  {t('users.actions.deactivate')}
                </DropdownMenuItem>
              ) : (
                <DropdownMenuItem onSelect={() => void activate(row.original)}>
                  {t('users.actions.activate')}
                </DropdownMenuItem>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
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
              disabled={usersQuery.isFetching}
              aria-label={t('users.actions.refresh')}
              onClick={() => void usersQuery.refetch()}
            >
              <RefreshCw aria-hidden="true" />
            </Button>
            <Button
              aria-label={t('users.actions.create')}
              onClick={() => setFormTarget(null)}
            >
              <Plus aria-hidden="true" />
              <span className="hidden sm:inline">
                {t('users.actions.create')}
              </span>
            </Button>
          </>
        }
      />

      <Card>
        <div className="flex flex-col gap-3 border-b p-4 md:flex-row md:items-center">
          <Input
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder={t('users.searchPlaceholder')}
            aria-label={t('users.searchPlaceholder')}
            className="md:max-w-xs"
          />
          <div className="flex flex-wrap items-center gap-2">
            <Select
              value={roleFilter}
              onValueChange={(value) => setRoleFilter(value as RoleFilter)}
            >
              <SelectTrigger
                className="w-36"
                aria-label={t('users.filters.allRoles')}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  {t('users.filters.allRoles')}
                </SelectItem>
                <SelectItem value="admin">
                  {t('users.filters.admins')}
                </SelectItem>
                <SelectItem value="user">{t('users.filters.users')}</SelectItem>
              </SelectContent>
            </Select>
            <Select
              value={statusFilter}
              onValueChange={(value) => setStatusFilter(value as StatusFilter)}
            >
              <SelectTrigger
                className="w-36"
                aria-label={t('users.filters.allStatuses')}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  {t('users.filters.allStatuses')}
                </SelectItem>
                <SelectItem value="active">
                  {t('users.filters.active')}
                </SelectItem>
                <SelectItem value="inactive">
                  {t('users.filters.inactive')}
                </SelectItem>
              </SelectContent>
            </Select>
            <span className="ml-auto text-sm text-muted-foreground">
              {t('users.count', { shown: rows.length, total })}
            </span>
          </div>
        </div>

        <CardContent className="p-0">
          {usersQuery.error ? (
            <div className="p-4">
              <ErrorAlert
                error={errorMessage(usersQuery.error, t('users.errors.load'))}
                onRetry={() => void usersQuery.refetch()}
              />
            </div>
          ) : (
            <DataTable
              columns={columns}
              data={rows}
              getRowId={(row) => String(row.user_id)}
              loading={usersQuery.isPending}
              emptyTitle={
                search ? t('users.empty.matchingTitle') : t('users.empty.title')
              }
              emptyDescription={
                search
                  ? t('users.empty.searchDescription')
                  : t('users.empty.description')
              }
              caption={t('users.table.user')}
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
        <UserFormDialog
          key={formTarget?.user_id ?? 'new'}
          user={formTarget}
          onOpenChange={(open) => {
            if (!open) setFormTarget(undefined)
          }}
        />
      ) : null}

      {resetTarget ? (
        <ResetPasswordDialog
          key={resetTarget.user_id}
          user={resetTarget}
          onOpenChange={(open) => {
            if (!open) setResetTarget(null)
          }}
        />
      ) : null}

      {tokensTarget ? (
        <UserTokensDialog
          key={tokensTarget.user_id}
          user={tokensTarget}
          onOpenChange={(open) => {
            if (!open) setTokensTarget(null)
          }}
        />
      ) : null}

      <ConfirmDialog
        open={deactivateTarget !== null}
        title={t('users.confirmations.deactivateTitle')}
        description={
          deactivateTarget
            ? t('users.confirmations.deactivateDescription', {
                displayName: deactivateTarget.display_name,
              })
            : undefined
        }
        confirmLabel={t('users.actions.deactivate')}
        cancelLabel={t('shared.confirm.cancel')}
        destructive
        pending={deactivateUser.isPending}
        onConfirm={() => void confirmDeactivate()}
        onOpenChange={(open) => {
          if (!open && !deactivateUser.isPending) setDeactivateTarget(null)
        }}
      />
    </div>
  )
}
