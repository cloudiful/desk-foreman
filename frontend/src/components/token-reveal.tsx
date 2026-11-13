import { Copy, X } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'

interface TokenRevealProps {
  token: string
  onDismiss: () => void
}

export function TokenReveal({ token, onDismiss }: TokenRevealProps) {
  const { t } = useTranslation()
  const [copied, setCopied] = useState(false)

  async function copyToken(): Promise<void> {
    try {
      await navigator.clipboard.writeText(token)
      setCopied(true)
      toast.success(t('shared.token.copied'))
    } catch {
      toast.error(t('shared.notifications.copyFailed'))
    }
  }

  function dismiss(): void {
    setCopied(false)
    onDismiss()
  }

  return (
    <Alert>
      <AlertTitle>{t('shared.token.copyTitle')}</AlertTitle>
      <AlertDescription className="space-y-3">
        <p>{t('shared.token.copyDescription')}</p>
        <code className="block overflow-x-auto rounded-md bg-muted px-3 py-2 font-mono text-xs break-all">
          {token}
        </code>
        <div className="flex flex-wrap gap-2">
          <Button type="button" size="sm" onClick={() => void copyToken()}>
            <Copy aria-hidden="true" />
            {copied
              ? t('shared.token.copied')
              : t('shared.token.copyAriaLabel')}
          </Button>
          <Button type="button" size="sm" variant="outline" onClick={dismiss}>
            <X aria-hidden="true" />
            {t('shared.confirm.cancel')}
          </Button>
        </div>
      </AlertDescription>
    </Alert>
  )
}
