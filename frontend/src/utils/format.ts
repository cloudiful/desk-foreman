import { i18n } from '../i18n'

export function formatDateTime(value: string | null | undefined): string {
  if (!value) return '—'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return '—'
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(date)
}

export function formatRelative(value: string | null | undefined): string {
  if (!value) return '—'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return '—'
  const diffMs = Date.now() - date.getTime()
  const seconds = Math.round(diffMs / 1000)
  let amount = seconds
  let unit: Intl.RelativeTimeFormatUnit = 'second'
  if (Math.abs(seconds) >= 60) {
    const minutes = Math.round(seconds / 60)
    amount = minutes
    unit = 'minute'
    if (Math.abs(minutes) >= 60) {
      const hours = Math.round(minutes / 60)
      amount = hours
      unit = 'hour'
      if (Math.abs(hours) >= 24) {
        amount = Math.round(hours / 24)
        unit = 'day'
      }
    }
  }
  return new Intl.RelativeTimeFormat(i18n.global.locale.value, {
    numeric: 'always',
  }).format(-amount, unit)
}

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined) return '—'
  if (bytes < 1024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let value = bytes
  let unit = 'B'
  for (const next of units) {
    if (value < 1024) break
    value /= 1024
    unit = next
  }
  return `${value >= 100 ? Math.round(value) : value.toFixed(1)} ${unit}`
}

export function formatDuration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) return '—'
  if (seconds < 60)
    return i18n.global.t('shared.duration.seconds', { value: seconds })
  const minutes = Math.floor(seconds / 60)
  const rest = seconds % 60
  if (minutes < 60)
    return `${i18n.global.t('shared.duration.minutes', { value: minutes })} ${i18n.global.t('shared.duration.seconds', { value: rest })}`
  const hours = Math.floor(minutes / 60)
  return `${i18n.global.t('shared.duration.hours', { value: hours })} ${i18n.global.t('shared.duration.minutes', { value: minutes % 60 })}`
}

export function formatMilliseconds(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return '—'
  return formatDuration(Math.round(ms / 1000))
}

export function truncateMiddle(value: string, max = 28): string {
  if (value.length <= max) return value
  const half = Math.floor((max - 1) / 2)
  return `${value.slice(0, half)}…${value.slice(-half)}`
}

export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean)
  if (!parts.length) return '?'
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase()
  return `${parts[0][0]}${parts[parts.length - 1][0]}`.toUpperCase()
}
