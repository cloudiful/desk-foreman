import {
  Activity,
  AppWindow,
  FolderTree,
  Hammer,
  KeyRound,
  LayoutDashboard,
  LogOut,
  Menu,
  Settings,
  ScrollText,
  ServerCog,
  Users,
  X,
  type LucideIcon,
} from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, Outlet, useLocation, useNavigate } from 'react-router'
import { toast } from 'sonner'
import { errorMessage } from '@/api/http'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/utils'
import { PageActionsContext } from './page-actions'
import { useSession } from '@/features/auth/session-context'

interface NavItem {
  path: string
  label: string
  icon: LucideIcon
  adminOnly: boolean
}

export function AppShell() {
  const { t } = useTranslation()
  const { user, logout } = useSession()
  const location = useLocation()
  const navigate = useNavigate()
  const [navOpen, setNavOpen] = useState(false)
  const [actionTarget, setActionTarget] = useState<HTMLDivElement | null>(null)

  const navItems: NavItem[] = [
    {
      path: '/',
      label: t('shell.navigation.overview'),
      icon: LayoutDashboard,
      adminOnly: false,
    },
    {
      path: '/admin/users',
      label: t('shell.navigation.users'),
      icon: Users,
      adminOnly: true,
    },
    {
      path: '/admin/applications',
      label: t('shell.navigation.applications'),
      icon: AppWindow,
      adminOnly: true,
    },
    {
      path: '/admin/runner-managers',
      label: t('shell.navigation.runnerManagers'),
      icon: ServerCog,
      adminOnly: true,
    },
    {
      path: '/admin/operations',
      label: t('shell.navigation.operations'),
      icon: Activity,
      adminOnly: true,
    },
    {
      path: '/admin/workspace-bindings',
      label: t('shell.navigation.bindings'),
      icon: FolderTree,
      adminOnly: true,
    },
    {
      path: '/admin/audit',
      label: t('shell.navigation.audit'),
      icon: ScrollText,
      adminOnly: true,
    },
  ]

  const visibleItems = navItems.filter(
    (item) => !item.adminOnly || user?.is_admin,
  )

  function isActive(path: string): boolean {
    if (path === '/') return location.pathname === '/'
    return (
      location.pathname === path || location.pathname.startsWith(`${path}/`)
    )
  }

  const title = isActive('/settings')
    ? t('shell.settings')
    : (visibleItems.find((item) => isActive(item.path))?.label ??
      t('shell.productName'))

  async function handleLogout(): Promise<void> {
    try {
      await logout()
    } catch (error) {
      toast.error(errorMessage(error, t('auth.errors.logoutFailed')))
    }
    navigate('/login', { replace: true })
  }

  const navList = (
    <nav className="space-y-1">
      {visibleItems.map((item) => {
        const Icon = item.icon
        const active = isActive(item.path)
        return (
          <Link
            key={item.path}
            to={item.path}
            onClick={() => setNavOpen(false)}
            aria-current={active ? 'page' : undefined}
            className={cn(
              'flex items-center gap-2.5 rounded-md px-3 py-2 text-sm font-medium transition-colors',
              active
                ? 'bg-primary/10 text-primary'
                : 'text-muted-foreground hover:bg-accent hover:text-accent-foreground',
            )}
          >
            <Icon className="size-4.5" aria-hidden="true" />
            {item.label}
          </Link>
        )
      })}
    </nav>
  )

  const footer = (
    <div className="border-t p-3">
      <Link
        to="/settings"
        onClick={() => setNavOpen(false)}
        className="flex items-center gap-2.5 rounded-md px-3 py-2 text-sm font-medium text-muted-foreground hover:bg-accent hover:text-accent-foreground"
      >
        <Settings className="size-4.5" aria-hidden="true" />
        {t('shell.settings')}
      </Link>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            className="flex w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            aria-label={t('shell.openUserMenu')}
          >
            <span className="flex size-8 shrink-0 items-center justify-center rounded-full bg-secondary text-xs font-medium text-secondary-foreground">
              {(user?.display_name ?? user?.login_name ?? t('shell.user'))
                .slice(0, 2)
                .toUpperCase()}
            </span>
            <span className="truncate">
              {user?.display_name ?? user?.login_name ?? t('shell.user')}
            </span>
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent side="top" align="start">
          <DropdownMenuLabel>
            {user?.display_name ?? user?.login_name ?? t('shell.user')}
          </DropdownMenuLabel>
          {user?.email ? (
            <div className="px-2 py-1 text-xs text-muted-foreground">
              {user.email}
            </div>
          ) : null}
          <DropdownMenuSeparator />
          <DropdownMenuItem
            onSelect={() => {
              setNavOpen(false)
              navigate('/change-password')
            }}
          >
            <KeyRound aria-hidden="true" />
            {t('shell.changePassword')}
          </DropdownMenuItem>
          <DropdownMenuItem
            className="text-destructive focus:text-destructive"
            onSelect={() => void handleLogout()}
          >
            <LogOut aria-hidden="true" />
            {t('shell.signOut')}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  )

  return (
    <PageActionsContext.Provider value={actionTarget}>
      <div className="min-h-screen bg-muted">
        <div className="mx-auto flex max-w-7xl">
          <aside className="sticky top-0 hidden h-screen w-64 shrink-0 flex-col border-r bg-card lg:flex">
            <div className="flex items-center gap-2.5 px-5 py-4">
              <div className="flex size-8 items-center justify-center rounded-md bg-primary text-primary-foreground">
                <Hammer className="size-4.5" aria-hidden="true" />
              </div>
              <div>
                <div className="text-sm font-semibold">
                  {t('shell.productName')}
                </div>
                <div className="text-xs text-muted-foreground">
                  {t('shell.subtitle')}
                </div>
              </div>
            </div>
            <div className="mt-2 flex-1 overflow-y-auto px-3 pb-4">
              {navList}
            </div>
            {footer}
          </aside>

          <div className="flex min-w-0 flex-1 flex-col">
            <header className="sticky top-0 z-20 flex items-center gap-3 border-b bg-card/80 px-4 py-3 backdrop-blur md:px-6">
              <Button
                variant="ghost"
                size="icon"
                className="lg:hidden"
                aria-label={t('shell.openNavigation')}
                onClick={() => setNavOpen((open) => !open)}
              >
                {navOpen ? (
                  <X aria-hidden="true" />
                ) : (
                  <Menu aria-hidden="true" />
                )}
              </Button>
              <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">
                {title}
              </h2>

              <div
                ref={setActionTarget}
                className="flex shrink-0 items-center gap-2"
              />
            </header>

            <main className="flex-1 px-4 py-6 md:px-6">
              <Outlet />
            </main>
          </div>
        </div>

        {navOpen ? (
          <div className="fixed inset-0 z-40 lg:hidden">
            <button
              type="button"
              className="absolute inset-0 bg-black/50"
              aria-label={t('shell.openNavigation')}
              onClick={() => setNavOpen(false)}
            />
            <div className="absolute inset-y-0 left-0 flex w-64 flex-col border-r bg-card">
              <div className="flex items-center gap-2.5 p-4">
                <div className="flex size-8 items-center justify-center rounded-md bg-primary text-primary-foreground">
                  <Hammer className="size-4.5" aria-hidden="true" />
                </div>
                <div className="text-sm font-semibold">
                  {t('shell.productName')}
                </div>
              </div>
              <div className="flex-1 overflow-y-auto px-3">{navList}</div>
              {footer}
            </div>
          </div>
        ) : null}
      </div>
    </PageActionsContext.Provider>
  )
}
