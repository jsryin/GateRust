import type { ReactNode } from 'react';
import { CircleCheck, CircleX } from 'lucide-react';
import { classNames } from '../../lib/class-names';

export function Notice({ children, tone }: { children: ReactNode; tone: 'success' | 'error' }) {
  const success = tone === 'success';
  const Icon = success ? CircleCheck : CircleX;

  return (
    <div
      className={classNames(
        'txt-compact-small mb-3 flex min-h-10 items-center gap-2 rounded-md border px-3 py-2',
        success
          ? 'border-[color:var(--tag-green-border)] bg-[var(--tag-green-bg)] text-[color:var(--tag-green-text)]'
          : 'border-[color:var(--tag-red-border)] bg-[var(--tag-red-bg)] text-[color:var(--tag-red-text)]'
      )}
      role={success ? 'status' : 'alert'}
    >
      <Icon className="h-4 w-4 shrink-0" />
      <div className="min-w-0 flex-1">{children}</div>
    </div>
  );
}
