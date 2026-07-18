import * as DialogPrimitive from '@radix-ui/react-dialog';
import { X } from 'lucide-react';
import type { ReactNode } from 'react';
import { classNames } from '../../lib/class-names';
import { Button } from './Button';

export const Dialog = DialogPrimitive.Root;

interface DialogContentProps {
  children: ReactNode;
  className?: string;
  title: string;
  description?: string;
}

export function DialogContent({ children, className, description, title }: DialogContentProps) {
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay
        className="fixed inset-0 z-50 bg-[var(--bg-overlay)]"
        data-radix-dialog-overlay
      />
      <DialogPrimitive.Content
        className={classNames(
          'fixed left-1/2 top-1/2 z-50 flex max-h-[calc(100vh-1rem)] w-[calc(100%-1rem)] max-w-[640px] -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-lg border border-[color:var(--border-base)] bg-[var(--bg-base)] text-[color:var(--fg-base)] shadow-[var(--elevation-modal)] outline-none',
          className
        )}
        data-radix-dialog-content
      >
        <header className="border-b border-[color:var(--border-base)] px-5 py-4 pr-14 sm:px-6">
          <DialogPrimitive.Title className="txt-compact-large-plus">{title}</DialogPrimitive.Title>
          {description && (
            <DialogPrimitive.Description className="txt-compact-small mt-1 text-[color:var(--fg-muted)]">
              {description}
            </DialogPrimitive.Description>
          )}
        </header>
        {children}
        <DialogPrimitive.Close asChild>
          <Button className="absolute right-4 top-3.5" size="icon" variant="ghost" aria-label="关闭">
            <X className="h-4 w-4" />
          </Button>
        </DialogPrimitive.Close>
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
}

export function DialogBody({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={classNames('min-h-0 flex-1 overflow-y-auto px-5 py-4 sm:px-6', className)}>{children}</div>;
}

export function DialogFooter({ children }: { children: ReactNode }) {
  return (
    <footer className="flex justify-end gap-2 border-t border-[color:var(--border-base)] px-5 py-4 sm:px-6">
      {children}
    </footer>
  );
}
