import { useContext, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { PageActionsContext } from '@/features/shell/page-actions'

interface PageHeaderProps {
  actions?: ReactNode
}

export function PageHeader({ actions }: PageHeaderProps) {
  const target = useContext(PageActionsContext)
  return target ? createPortal(actions, target) : null
}
