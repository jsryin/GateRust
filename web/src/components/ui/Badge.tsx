import type { ReactNode } from 'react';
import { classNames } from '../../lib/class-names';

type BadgeTone = 'green' | 'orange' | 'neutral' | 'red';

const tones: Record<BadgeTone, string> = {
  green: 'border-[color:var(--tag-green-border)] bg-[var(--tag-green-bg)] text-[color:var(--tag-green-text)]',
  orange: 'border-[color:var(--tag-orange-border)] bg-[var(--tag-orange-bg)] text-[color:var(--tag-orange-text)]',
  neutral: 'border-[color:var(--tag-neutral-border)] bg-[var(--tag-neutral-bg)] text-[color:var(--tag-neutral-text)]',
  red: 'border-[color:var(--tag-red-border)] bg-[var(--tag-red-bg)] text-[color:var(--tag-red-text)]'
};

export function Badge({ children, tone = 'neutral' }: { children: ReactNode; tone?: BadgeTone }) {
  return (
    <span className={classNames('txt-compact-xsmall-plus inline-flex rounded-md border px-1.5 py-0.5', tones[tone])}>
      {children}
    </span>
  );
}
