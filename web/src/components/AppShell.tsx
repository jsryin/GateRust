import {
  Activity,
  Cable,
  FileCode2,
  LogOut,
  Menu,
  Network,
  ShieldCheck,
  X
} from 'lucide-react';
import { useState, type ReactNode } from 'react';
import type { Theme } from '../hooks/useTheme';
import { classNames } from '../lib/class-names';
import { ThemeButton } from './ThemeButton';
import { Button } from './ui/Button';

export type PageId = 'dashboard' | 'tunnel' | 'proxy' | 'client';

const pages: Array<{ id: PageId; label: string; icon: typeof Activity }> = [
  { id: 'dashboard', label: '仪表盘', icon: Activity },
  { id: 'tunnel', label: '分组与隧道', icon: Cable },
  { id: 'proxy', label: '域名与 SSL', icon: ShieldCheck },
  { id: 'client', label: '客户端配置', icon: FileCode2 }
];

interface AppShellProps {
  active: PageId;
  children: ReactNode;
  onLogout: () => void;
  onNavigate: (page: PageId) => void;
  onToggleTheme: () => void;
  theme: Theme;
}

export function AppShell({ active, children, onLogout, onNavigate, onToggleTheme, theme }: AppShellProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const activePage = pages.find((page) => page.id === active) ?? pages[0];

  function navigate(page: PageId) {
    onNavigate(page);
    setMenuOpen(false);
  }

  return (
    <div className="min-h-screen bg-[var(--bg-subtle)] text-[color:var(--fg-base)]">
      {menuOpen && (
        <button
          aria-label="关闭导航"
          className="fixed inset-0 z-40 bg-[var(--bg-overlay)] lg:hidden"
          onClick={() => setMenuOpen(false)}
          type="button"
        />
      )}
      <aside
        className={classNames(
          'fixed inset-y-0 left-0 z-50 flex w-[244px] flex-col border-r border-[color:var(--border-base)] bg-[var(--bg-base)] px-3 py-3 transition-transform duration-200 lg:translate-x-0',
          menuOpen ? 'visible translate-x-0' : 'invisible -translate-x-full lg:visible'
        )}
      >
        <div className="flex h-10 items-center gap-2 px-2">
          <div className="grid h-7 w-7 place-items-center rounded-md bg-[var(--button-neutral)] text-[color:var(--fg-subtle)] shadow-[var(--buttons-neutral)]">
            <Network className="h-4 w-4" />
          </div>
          <div className="min-w-0">
            <div className="txt-compact-small-plus truncate">GateRust</div>
            <div className="txt-compact-xsmall text-[color:var(--fg-muted)]">中心控制台</div>
          </div>
          <Button
            aria-label="关闭导航"
            className="ml-auto lg:hidden"
            onClick={() => setMenuOpen(false)}
            size="icon"
            variant="ghost"
          >
            <X className="h-4 w-4" />
          </Button>
        </div>

        <div className="mt-4 flex h-10 w-full items-center justify-between rounded-md bg-[var(--button-neutral)] px-2.5 shadow-[var(--buttons-neutral)]">
          <span className="min-w-0">
            <span className="txt-compact-small-plus block truncate">服务端</span>
            <span className="txt-compact-xsmall block truncate text-[color:var(--fg-muted)]">当前运行实例</span>
          </span>
          <span className="h-2 w-2 rounded-full bg-emerald-500" />
        </div>

        <div className="txt-compact-xsmall-plus mt-5 px-2 text-[color:var(--fg-muted)]">导航</div>
        <nav className="mt-2 space-y-1" aria-label="主导航">
          {pages.map((page) => {
            const Icon = page.icon;
            const selected = active === page.id;

            return (
              <button
                aria-current={selected ? 'page' : undefined}
                className={classNames(
                  'transition-fg txt-compact-small flex h-8 w-full items-center gap-2 rounded-md px-2 text-left',
                  selected
                    ? 'bg-[var(--bg-subtle)] font-medium text-[color:var(--fg-base)]'
                    : 'text-[color:var(--fg-subtle)] hover:bg-[var(--bg-base-hover)] hover:text-[color:var(--fg-base)]'
                )}
                key={page.id}
                onClick={() => navigate(page.id)}
                type="button"
              >
                <Icon className="h-4 w-4" />
                {page.label}
              </button>
            );
          })}
        </nav>

        <div className="txt-compact-xsmall mt-auto rounded-lg bg-[var(--bg-base)] p-3 shadow-[var(--elevation-card-rest)]">
          <div className="flex items-center gap-2 font-medium">
            <span className="h-2 w-2 rounded-full bg-emerald-500" />
            控制平面在线
          </div>
          <div className="mt-1 text-[color:var(--fg-muted)]">GateRust Web API</div>
        </div>
        <Button className="mt-3 w-full justify-start" onClick={onLogout} variant="ghost">
          <LogOut className="h-4 w-4" />
          退出登录
        </Button>
      </aside>

      <main className="lg:pl-[244px]">
        <header className="sticky top-0 z-30 border-b border-[color:var(--border-base)] bg-[var(--bg-base)]/95 backdrop-blur">
          <div className="flex min-h-14 items-center justify-between gap-3 px-4 py-2 lg:px-6">
            <div className="flex min-w-0 items-center gap-3">
              <Button
                aria-label="打开导航"
                className="lg:hidden"
                onClick={() => setMenuOpen(true)}
                size="icon"
                variant="secondary"
              >
                <Menu className="h-4 w-4" />
              </Button>
              <div className="min-w-0">
                <div className="txt-compact-xsmall truncate text-[color:var(--fg-muted)]">控制台 / {activePage.label}</div>
                <h1 className="txt-compact-large-plus truncate">{activePage.label}</h1>
              </div>
            </div>
            <ThemeButton onToggle={onToggleTheme} theme={theme} />
          </div>
        </header>
        <div className="mx-auto max-w-[1400px] px-4 py-5 lg:px-6">{children}</div>
      </main>
    </div>
  );
}
