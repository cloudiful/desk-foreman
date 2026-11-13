import { useEffect, useState } from 'react'

export function useDocumentVisibility(): boolean {
  const [hidden, setHidden] = useState(
    () => typeof document === 'undefined' || document.hidden,
  )

  useEffect(() => {
    const handleChange = () => setHidden(document.hidden)
    document.addEventListener('visibilitychange', handleChange)
    return () => document.removeEventListener('visibilitychange', handleChange)
  }, [])

  return hidden
}
