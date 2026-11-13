import { Trash2 } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { StatusBadge } from '@/components/status-badge'
import { Button } from '@/components/ui/button'
import { formatDateTime } from '@/utils/format'

export interface TokenCardItem {
  token_id: number
  token_name: string
  created_at: string
  expires_at?: string | null
  is_active: boolean
  scopes: string[]
}

interface TokenCardsProps<T extends TokenCardItem> {
  tokens: T[]
  emptyLabel: string
  onRevoke: (token: T) => void
}

function tokenStatus(token: TokenCardItem): string {
  if (!token.is_active) return 'disabled'
  if (token.expires_at) {
    const expires = new Date(token.expires_at).getTime()
    if (!Number.isNaN(expires) && expires <= Date.now()) return 'expired'
  }
  return 'active'
}

export function TokenCards<T extends TokenCardItem>({
  tokens,
  emptyLabel,
  onRevoke,
}: TokenCardsProps<T>) {
  const { t } = useTranslation()

  if (!tokens.length) {
    return <p className="text-sm text-muted-foreground">{emptyLabel}</p>
  }

  return (
    <div className="space-y-2">
      {tokens.map((token) => (
        <div
          key={token.token_id}
          className="flex items-start justify-between gap-3 rounded-lg border px-3 py-2.5"
        >
          <div className="min-w-0 space-y-1">
            <div className="flex flex-wrap items-center gap-2">
              <span className="truncate text-sm font-medium">
                {token.token_name}
              </span>
              <StatusBadge status={tokenStatus(token)} />
            </div>
            <div className="text-xs text-muted-foreground">
              {t('shared.token.created', {
                time: formatDateTime(token.created_at),
              })}
              {token.expires_at ? (
                <>
                  {' · '}
                  {t('shared.token.expires', {
                    time: formatDateTime(token.expires_at),
                  })}
                </>
              ) : null}
            </div>
            <div className="font-mono text-xs text-muted-foreground">
              {token.scopes.join(', ') || t('shared.token.noScopes')}
            </div>
          </div>
          <Button
            type="button"
            size="icon"
            variant="ghost"
            className="text-destructive hover:text-destructive"
            aria-label={t('shared.token.revokeAriaLabel', {
              tokenName: token.token_name,
            })}
            onClick={() => onRevoke(token)}
          >
            <Trash2 aria-hidden="true" />
          </Button>
        </div>
      ))}
    </div>
  )
}
