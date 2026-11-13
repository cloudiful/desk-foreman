import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ColumnDef } from '@tanstack/react-table'
import { Pencil, Plus, ToggleLeft, ToggleRight, Trash2 } from 'lucide-react'
import { useCallback, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  deleteWorkspaceGitUpstreamMutation,
  disableWorkspaceGitUpstreamMutation,
  enableWorkspaceGitUpstreamMutation,
  listWorkspaceGitUpstreamsOptions,
  listWorkspaceGitUpstreamsQueryKey,
  type WorkspaceBindingResponse,
  type WorkspaceGitUpstreamResponse,
} from '@/api/generated'
import { errorMessage } from '@/api/http'
import { ConfirmDialog } from '@/components/confirm-dialog'
import { DataTable } from '@/components/data-table'
import { ErrorAlert } from '@/components/error-alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { UpstreamForm } from './upstream-form'

interface WorkspaceGitUpstreamsDialogProps {
  binding: WorkspaceBindingResponse
  onOpenChange: (open: boolean) => void
}

export function WorkspaceGitUpstreamsDialog({
  binding,
  onOpenChange,
}: WorkspaceGitUpstreamsDialogProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [editing, setEditing] = useState<WorkspaceGitUpstreamResponse | null>(
    null,
  )
  const [formVersion, setFormVersion] = useState(0)
  const [formSubmitting, setFormSubmitting] = useState(false)
  const formSubmittingRef = useRef(false)
  const [deleteTarget, setDeleteTarget] =
    useState<WorkspaceGitUpstreamResponse | null>(null)
  const [togglingId, setTogglingId] = useState<number | null>(null)

  const readOnly = binding.lifecycle_state !== 'active' || !binding.is_active

  const upstreamsQuery = useQuery(
    listWorkspaceGitUpstreamsOptions({
      path: { binding_id: binding.workspace_binding_id },
      query: { limit: 200, offset: 0 },
    }),
  )

  const enableUpstream = useMutation({
    ...enableWorkspaceGitUpstreamMutation(),
  })
  const disableUpstream = useMutation({
    ...disableWorkspaceGitUpstreamMutation(),
  })
  const deleteUpstream = useMutation({
    ...deleteWorkspaceGitUpstreamMutation(),
  })

  const handleSubmittingChange = useCallback((submitting: boolean) => {
    formSubmittingRef.current = submitting
    setFormSubmitting(submitting)
  }, [])

  function resetForm(force = false): void {
    if (formSubmittingRef.current && !force) return
    setEditing(null)
    setFormVersion((version) => version + 1)
  }

  function startEdit(row: WorkspaceGitUpstreamResponse): void {
    if (formSubmittingRef.current) return
    setEditing(row)
    setFormVersion((version) => version + 1)
  }

  function closeDialog(): void {
    if (formSubmittingRef.current) return
    onOpenChange(false)
  }

  async function invalidateList(): Promise<void> {
    await queryClient.invalidateQueries({
      queryKey: listWorkspaceGitUpstreamsQueryKey({
        path: { binding_id: binding.workspace_binding_id },
      }),
    })
  }

  async function toggle(row: WorkspaceGitUpstreamResponse): Promise<void> {
    if (readOnly || formSubmittingRef.current) return
    setTogglingId(row.upstream_id)
    const mutation = row.enabled ? disableUpstream : enableUpstream
    try {
      await mutation.mutateAsync({
        path: {
          binding_id: binding.workspace_binding_id,
          upstream_id: row.upstream_id,
        },
      })
      toast.success(
        row.enabled
          ? t('workspaceBindings.upstreams.notifications.disabled')
          : t('workspaceBindings.upstreams.notifications.enabled'),
      )
      await invalidateList()
    } catch (error) {
      toast.error(t('workspaceBindings.errors.toggleUpstream'), {
        description: errorMessage(
          error,
          t('workspaceBindings.errors.toggleUpstream'),
        ),
      })
    } finally {
      setTogglingId(null)
    }
  }

  async function confirmDelete(): Promise<void> {
    if (!deleteTarget || formSubmittingRef.current) return
    try {
      await deleteUpstream.mutateAsync({
        path: {
          binding_id: binding.workspace_binding_id,
          upstream_id: deleteTarget.upstream_id,
        },
      })
      toast.success(t('workspaceBindings.upstreams.notifications.deleted'))
      if (editing?.upstream_id === deleteTarget.upstream_id) resetForm()
      setDeleteTarget(null)
      await invalidateList()
    } catch (error) {
      toast.error(t('workspaceBindings.errors.deleteUpstream'), {
        description: errorMessage(
          error,
          t('workspaceBindings.errors.deleteUpstream'),
        ),
      })
    }
  }

  const columns: ColumnDef<WorkspaceGitUpstreamResponse, unknown>[] = [
    {
      accessorKey: 'remote_alias',
      header: t('workspaceBindings.upstreams.table.alias'),
      cell: ({ row }) => (
        <span className="font-mono text-sm">{row.original.remote_alias}</span>
      ),
    },
    {
      accessorKey: 'target_url',
      header: t('workspaceBindings.upstreams.table.targetUrl'),
      cell: ({ row }) => (
        <span className="block max-w-md truncate font-mono text-xs">
          {row.original.target_url}
        </span>
      ),
    },
    {
      id: 'enabled',
      header: t('workspaceBindings.upstreams.table.enabled'),
      cell: ({ row }) => (
        <Badge variant="secondary">
          {row.original.enabled
            ? t('workspaceBindings.upstreams.state.enabled')
            : t('workspaceBindings.upstreams.state.disabled')}
        </Badge>
      ),
    },
    {
      id: 'credential',
      header: t('workspaceBindings.upstreams.table.credential'),
      cell: ({ row }) => (
        <span className="text-sm text-muted-foreground">
          {row.original.has_credential
            ? t('workspaceBindings.upstreams.credential.present')
            : t('workspaceBindings.upstreams.credential.absent')}
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
            disabled={readOnly || formSubmitting}
            aria-label={t('workspaceBindings.upstreams.actions.edit')}
            onClick={() => startEdit(row.original)}
          >
            <Pencil aria-hidden="true" />
          </Button>
          <Button
            size="icon"
            variant="ghost"
            disabled={
              readOnly ||
              formSubmitting ||
              togglingId === row.original.upstream_id
            }
            aria-label={
              row.original.enabled
                ? t('workspaceBindings.upstreams.actions.disable')
                : t('workspaceBindings.upstreams.actions.enable')
            }
            onClick={() => void toggle(row.original)}
          >
            {row.original.enabled ? (
              <ToggleRight aria-hidden="true" />
            ) : (
              <ToggleLeft aria-hidden="true" />
            )}
          </Button>
          <Button
            size="icon"
            variant="ghost"
            className="text-destructive hover:text-destructive"
            disabled={readOnly || formSubmitting}
            aria-label={t('workspaceBindings.upstreams.actions.delete')}
            onClick={() => setDeleteTarget(row.original)}
          >
            <Trash2 aria-hidden="true" />
          </Button>
        </div>
      ),
    },
  ]

  const rows = upstreamsQuery.data?.items ?? []

  return (
    <>
      <Dialog
        open
        onOpenChange={(next) => {
          if (!next) closeDialog()
        }}
      >
        <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-4xl">
          <DialogHeader>
            <DialogTitle>
              {t('workspaceBindings.upstreams.title', {
                id: binding.workspace_binding_id,
              })}
            </DialogTitle>
            <DialogDescription>
              {t('workspaceBindings.upstreams.hint')}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-6">
            {readOnly ? (
              <Alert>
                <AlertTitle>
                  {t('workspaceBindings.upstreams.archived.title')}
                </AlertTitle>
                <AlertDescription>
                  {t('workspaceBindings.upstreams.archived.description')}
                </AlertDescription>
              </Alert>
            ) : null}

            {upstreamsQuery.error ? (
              <ErrorAlert
                error={errorMessage(
                  upstreamsQuery.error,
                  t('workspaceBindings.errors.loadUpstreams'),
                )}
                onRetry={() => void upstreamsQuery.refetch()}
              />
            ) : null}

            <div className="flex items-center justify-end">
              <Button
                size="sm"
                disabled={readOnly || formSubmitting}
                aria-label={t('workspaceBindings.upstreams.actions.add')}
                onClick={() => resetForm()}
              >
                <Plus aria-hidden="true" />
                {t('workspaceBindings.upstreams.actions.add')}
              </Button>
            </div>

            <DataTable
              columns={columns}
              data={rows}
              getRowId={(row) => String(row.upstream_id)}
              loading={upstreamsQuery.isPending}
              emptyTitle={t('workspaceBindings.upstreams.empty.title')}
              emptyDescription={t(
                'workspaceBindings.upstreams.empty.description',
              )}
              caption={t('workspaceBindings.upstreams.table.alias')}
            />

            {!readOnly ? (
              <UpstreamForm
                key={`${editing?.upstream_id ?? 'new'}-${formVersion}`}
                bindingId={binding.workspace_binding_id}
                upstream={editing}
                onCancel={() => resetForm()}
                onSubmittingChange={handleSubmittingChange}
                onSaved={() => {
                  resetForm(true)
                  void invalidateList()
                }}
              />
            ) : null}
          </div>

          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={formSubmitting}
              onClick={closeDialog}
            >
              {t('shared.confirm.cancel')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        open={deleteTarget !== null}
        title={t('workspaceBindings.upstreams.confirmDelete.title')}
        description={
          deleteTarget
            ? t('workspaceBindings.upstreams.confirmDelete.description', {
                alias: deleteTarget.remote_alias,
              })
            : undefined
        }
        confirmLabel={t('workspaceBindings.upstreams.actions.delete')}
        cancelLabel={t('shared.confirm.cancel')}
        destructive
        pending={deleteUpstream.isPending}
        onConfirm={() => void confirmDelete()}
        onOpenChange={(open) => {
          if (!open && !deleteUpstream.isPending) setDeleteTarget(null)
        }}
      />
    </>
  )
}
