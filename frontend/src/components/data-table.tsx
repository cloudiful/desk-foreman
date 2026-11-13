import {
  flexRender,
  getCoreRowModel,
  useReactTable,
  type ColumnDef,
} from '@tanstack/react-table'
import { useTranslation } from 'react-i18next'
import { EmptyState } from '@/components/empty-state'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'

interface DataTableProps<TData> {
  columns: ColumnDef<TData, unknown>[]
  data: TData[]
  getRowId: (row: TData, index: number) => string
  loading?: boolean
  emptyTitle?: string
  emptyDescription?: string
  caption?: string
}

export function DataTable<TData>({
  columns,
  data,
  getRowId,
  loading = false,
  emptyTitle,
  emptyDescription,
  caption,
}: DataTableProps<TData>) {
  const { t } = useTranslation()

  const table = useReactTable({
    data,
    columns,
    getCoreRowModel: getCoreRowModel(),
    getRowId: (row, index) => getRowId(row, index),
    manualPagination: true,
  })

  const rows = table.getRowModel().rows

  return (
    <Table>
      {caption ? <caption className="sr-only">{caption}</caption> : null}
      <TableHeader>
        {table.getHeaderGroups().map((headerGroup) => (
          <TableRow key={headerGroup.id}>
            {headerGroup.headers.map((header) => (
              <TableHead key={header.id}>
                {header.isPlaceholder
                  ? null
                  : flexRender(
                      header.column.columnDef.header,
                      header.getContext(),
                    )}
              </TableHead>
            ))}
          </TableRow>
        ))}
      </TableHeader>
      <TableBody>
        {loading && rows.length === 0
          ? Array.from({ length: 3 }, (_, index) => (
              <TableRow key={`skeleton-${index}`}>
                {columns.map((column, cellIndex) => (
                  <TableCell key={cellIndex}>
                    <Skeleton className="h-4 w-full" />
                  </TableCell>
                ))}
              </TableRow>
            ))
          : null}
        {rows.map((row) => (
          <TableRow key={row.id}>
            {row.getVisibleCells().map((cell) => (
              <TableCell key={cell.id}>
                {flexRender(cell.column.columnDef.cell, cell.getContext())}
              </TableCell>
            ))}
          </TableRow>
        ))}
        {!loading && rows.length === 0 ? (
          <TableRow>
            <TableCell colSpan={columns.length}>
              <EmptyState
                title={emptyTitle ?? t('shared.empty.noData')}
                description={emptyDescription}
              />
            </TableCell>
          </TableRow>
        ) : null}
      </TableBody>
    </Table>
  )
}
