import type { TokenScope } from '@/features/access/scopes'
import { TOKEN_SCOPES } from '@/features/access/scopes'
import { Checkbox } from '@/components/ui/checkbox'
import { Label } from '@/components/ui/label'

interface ScopeCheckboxGroupProps {
  idPrefix: string
  value: TokenScope[]
  onChange: (value: TokenScope[]) => void
  disabled?: boolean
}

export function ScopeCheckboxGroup({
  idPrefix,
  value,
  onChange,
  disabled = false,
}: ScopeCheckboxGroupProps) {
  function toggle(scope: TokenScope, checked: boolean): void {
    onChange(
      checked ? [...value, scope] : value.filter((item) => item !== scope),
    )
  }

  return (
    <div className="grid gap-2 sm:grid-cols-2" data-slot="checkbox-group">
      {TOKEN_SCOPES.map((scope) => {
        const inputId = `${idPrefix}-${scope.replace('.', '-')}`
        return (
          <div key={scope} className="flex items-center gap-2">
            <Checkbox
              id={inputId}
              checked={value.includes(scope)}
              disabled={disabled}
              onCheckedChange={(checked) => toggle(scope, checked === true)}
            />
            <Label htmlFor={inputId} className="font-mono text-xs font-normal">
              {scope}
            </Label>
          </div>
        )
      })}
    </div>
  )
}
