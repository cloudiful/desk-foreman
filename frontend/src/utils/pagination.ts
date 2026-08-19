export const PAGE_SIZE = 100
export const AUDIT_PAGE_SIZE = 50

export type PageLike<T> = {
  items: T[]
  total: number
  limit: number
  offset: number
}

export function emptyPage<T>(limit: number, offset: number): PageLike<T> {
  return { items: [], total: 0, limit, offset }
}

export function pageCount(total: number, pageSize: number): number {
  return total > 0 ? Math.max(1, Math.ceil(total / pageSize)) : 1
}

export function pageOffset(pageIndex: number, pageSize: number): number {
  return (Math.max(1, pageIndex) - 1) * pageSize
}
