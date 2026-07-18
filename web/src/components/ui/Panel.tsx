import type { ReactNode } from 'react';
import type { LucideIcon } from 'lucide-react';
import { classNames } from '../../lib/class-names';

export function Panel({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <section
      className={classNames(
        'overflow-hidden rounded-lg bg-[var(--bg-base)] shadow-[var(--elevation-card-rest)]',
        className
      )}
    >
      {children}
    </section>
  );
}

interface PanelHeaderProps {
  action?: ReactNode;
  description?: string;
  title: string;
}

export function PanelHeader({ action, description, title }: PanelHeaderProps) {
  return (
    <header className="flex min-h-16 flex-col gap-3 border-b border-[color:var(--border-base)] px-5 py-3.5 sm:flex-row sm:items-center sm:justify-between sm:px-6">
      <div className="min-w-0">
        <h2 className="txt-compact-medium-plus font-medium text-[color:var(--fg-base)]">{title}</h2>
        {description && <p className="txt-compact-xsmall mt-0.5 text-[color:var(--fg-muted)]">{description}</p>}
      </div>
      {action}
    </header>
  );
}

interface EmptyStateProps {
  description?: string;
  icon?: LucideIcon;
  title: string;
}

export function EmptyState({ description, icon: Icon, title }: EmptyStateProps) {
  return (
    <div className="flex min-h-40 flex-col items-center justify-center px-6 py-8 text-center">
      {Icon && (
        <div className="mb-3 grid h-9 w-9 place-items-center rounded-md bg-[var(--bg-component)] text-[color:var(--fg-muted)] shadow-[var(--borders-base)]">
          <Icon className="h-4 w-4" />
        </div>
      )}
      <strong className="txt-compact-small-plus font-medium">{title}</strong>
      {description && <p className="txt-compact-xsmall mt-1 max-w-md text-[color:var(--fg-muted)]">{description}</p>}
    </div>
  );
}
