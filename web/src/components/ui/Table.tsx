import type { HTMLAttributes, ReactNode, TableHTMLAttributes, TdHTMLAttributes, ThHTMLAttributes } from 'react';
import { classNames } from '../../lib/class-names';

export function Table({ className, ...props }: TableHTMLAttributes<HTMLTableElement>) {
  return (
    <div className="overflow-x-auto">
      <table className={classNames('w-full border-collapse text-left', className)} {...props} />
    </div>
  );
}

export function TableHeader({ children }: { children: ReactNode }) {
  return <thead className="bg-[var(--bg-component)]">{children}</thead>;
}

export function TableRow({ className, ...props }: HTMLAttributes<HTMLTableRowElement>) {
  return (
    <tr
      className={classNames(
        'border-b border-[color:var(--border-base)] last:border-b-0 hover:bg-[var(--bg-subtle)]',
        className
      )}
      {...props}
    />
  );
}

export function TableHead({ className, ...props }: ThHTMLAttributes<HTMLTableCellElement>) {
  return (
    <th
      className={classNames(
        'txt-compact-xsmall-plus h-10 whitespace-nowrap px-4 text-[color:var(--fg-muted)] first:pl-6 last:pr-6',
        className
      )}
      {...props}
    />
  );
}

export function TableCell({ className, ...props }: TdHTMLAttributes<HTMLTableCellElement>) {
  return (
    <td
      className={classNames(
        'txt-compact-small h-12 whitespace-nowrap px-4 text-[color:var(--fg-subtle)] first:pl-6 last:pr-6',
        className
      )}
      {...props}
    />
  );
}
