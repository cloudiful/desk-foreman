import { useTranslation } from 'react-i18next'
import { Switch } from '@/components/ui/switch'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { SUPPORTED_LOCALES, setLocale, type Locale } from '@/i18n'
import { useTheme } from '@/lib/theme-context'

export function SettingsPage() {
  const { t, i18n } = useTranslation()
  const { isDark, toggle } = useTheme()
  return (
    <div className="max-w-lg space-y-6">
      <div className="flex items-center justify-between gap-4">
        <label htmlFor="dark-mode" className="text-sm font-medium">
          {t('shell.darkMode')}
        </label>
        <Switch id="dark-mode" checked={isDark} onCheckedChange={toggle} />
      </div>
      <div className="flex items-center justify-between gap-4">
        <span id="language-label" className="text-sm font-medium">
          {t('shell.language')}
        </span>
        <Select
          value={i18n.language}
          onValueChange={(value) => setLocale(value as Locale)}
        >
          <SelectTrigger aria-labelledby="language-label" className="w-48">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {SUPPORTED_LOCALES.map((locale) => (
              <SelectItem key={locale} value={locale}>
                {t(`shell.languages.${locale === 'en-US' ? 'enUS' : 'zhCN'}`)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    </div>
  )
}
