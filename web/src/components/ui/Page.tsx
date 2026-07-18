import type { ReactNode } from 'react';

interface PageIntroProps {
  action?: ReactNode;
  description: string;
  title: string;
}

export function PageIntro({ action, description, title }: PageIntroProps) {
  return (
    <div className="mb-4 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
      <div>
        <h2 className="txt-compact-large-plus">{title}</h2>
        <p className="txt-compact-small mt-0.5 text-[color:var(--fg-muted)]">{description}</p>
      </div>
      {action}
    </div>
  );
}

export function FormGrid({ children, columns = 2 }: { children: ReactNode; columns?: 2 | 3 | 4 }) {
  const layout = {
    2: 'sm:grid-cols-2',
    3: 'sm:grid-cols-2 xl:grid-cols-3',
    4: 'sm:grid-cols-2 xl:grid-cols-4'
  }[columns];

  return <div className={`grid gap-4 px-5 py-4 sm:px-6 ${layout}`}>{children}</div>;
}
