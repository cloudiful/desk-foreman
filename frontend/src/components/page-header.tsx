import type { ReactNode } from 'react'

interface PageHeaderProps {
  actions?: ReactNode
}

export function PageHeader({ actions }: PageHeaderProps) {
  return (
    <div className="flex min-h-9 items-center justify-end gap-2">{actions}</div>
  )
}
