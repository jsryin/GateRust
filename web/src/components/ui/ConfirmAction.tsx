import * as AlertDialog from '@radix-ui/react-alert-dialog';
import type { ReactElement, ReactNode } from 'react';
import { Button } from './Button';

interface ConfirmActionProps {
  children: ReactElement;
  confirmLabel?: string;
  description?: ReactNode;
  onConfirm: () => void | Promise<void>;
  title: string;
}

export function ConfirmAction({
  children,
  confirmLabel = '确认',
  description,
  onConfirm,
  title
}: ConfirmActionProps) {
  return (
    <AlertDialog.Root>
      <AlertDialog.Trigger asChild>{children}</AlertDialog.Trigger>
      <AlertDialog.Portal>
        <AlertDialog.Overlay
          className="fixed inset-0 z-[60] bg-[var(--bg-overlay)]"
          data-radix-alert-overlay
        />
        <AlertDialog.Content
          className="fixed left-1/2 top-1/2 z-[60] w-[calc(100%-1rem)] max-w-md -translate-x-1/2 -translate-y-1/2 overflow-hidden rounded-lg border border-[color:var(--border-base)] bg-[var(--bg-base)] shadow-[var(--elevation-modal)] outline-none"
          data-radix-alert-content
        >
          <div className="px-6 py-5">
            <AlertDialog.Title className="txt-compact-large-plus">{title}</AlertDialog.Title>
            <AlertDialog.Description
              className={description ? 'txt-compact-small mt-2 text-[color:var(--fg-subtle)]' : 'sr-only'}
            >
              {description ?? '请确认是否继续。'}
            </AlertDialog.Description>
          </div>
          <div className="flex justify-end gap-2 border-t border-[color:var(--border-base)] px-6 py-4">
            <AlertDialog.Cancel asChild>
              <Button variant="secondary">取消</Button>
            </AlertDialog.Cancel>
            <AlertDialog.Action asChild>
              <Button variant="danger" onClick={() => void onConfirm()}>
                {confirmLabel}
              </Button>
            </AlertDialog.Action>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}
