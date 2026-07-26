import type { ReactNode } from 'react';
import { CircleAlert, CircleCheck, CircleX } from 'lucide-react';
import { classNames } from '../../lib/class-names';

export function Notice({ children, tone }: { children: ReactNode; tone: 'success' | 'warning' | 'error' }) {
  const success = tone === 'success';
  const warning = tone === 'warning';
  const Icon = success ? CircleCheck : warning ? CircleAlert : CircleX;

  return (
    <div
      className={classNames(
        'txt-compact-small mb-3 flex min-h-10 items-center gap-2 rounded-md border px-3 py-2',
        success
          ? 'border-[color:var(--tag-green-border)] bg-[var(--tag-green-bg)] text-[color:var(--tag-green-text)]'
          : warning
            ? 'border-[color:var(--tag-orange-border)] bg-[var(--tag-orange-bg)] text-[color:var(--tag-orange-text)]'
          : 'border-[color:var(--tag-red-border)] bg-[var(--tag-red-bg)] text-[color:var(--tag-red-text)]'
      )}
      role={tone === 'error' ? 'alert' : 'status'}
    >
      <Icon className="h-4 w-4 shrink-0" />
      <div className="min-w-0 flex-1">{children}</div>
    </div>
  );
}
