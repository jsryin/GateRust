import {
  forwardRef,
  type InputHTMLAttributes,
  type ReactNode,
  type SelectHTMLAttributes,
  type TextareaHTMLAttributes
} from 'react';
import { classNames } from '../../lib/class-names';

const controlClass =
  'transition-fg txt-compact-small h-8 w-full min-w-0 rounded-md border-0 bg-[var(--bg-field)] px-2.5 text-[color:var(--fg-base)] shadow-[var(--borders-base)] outline-none placeholder:text-[color:var(--fg-muted)] hover:bg-[var(--bg-field-hover)] focus:bg-[var(--bg-base)] focus:shadow-[var(--borders-focus)] disabled:cursor-not-allowed disabled:bg-[var(--bg-disabled)] disabled:text-[color:var(--fg-disabled)]';

export const Input = forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement>>(
  ({ className, ...props }, ref) => (
    <input ref={ref} className={classNames(controlClass, className)} {...props} />
  )
);

Input.displayName = 'Input';

export const Select = forwardRef<HTMLSelectElement, SelectHTMLAttributes<HTMLSelectElement>>(
  ({ className, ...props }, ref) => (
    <select ref={ref} className={classNames(controlClass, 'appearance-auto pr-7', className)} {...props} />
  )
);

Select.displayName = 'Select';

export const Textarea = forwardRef<HTMLTextAreaElement, TextareaHTMLAttributes<HTMLTextAreaElement>>(
  ({ className, ...props }, ref) => (
    <textarea
      ref={ref}
      className={classNames(controlClass, 'h-auto min-h-20 resize-y py-2', className)}
      {...props}
    />
  )
);

Textarea.displayName = 'Textarea';

interface FieldProps {
  children: ReactNode;
  className?: string;
  htmlFor?: string;
  label: string;
}

export function Field({ children, className, htmlFor, label }: FieldProps) {
  return (
    <label
      className={classNames('txt-compact-small-plus grid min-w-0 content-start gap-1.5 text-[color:var(--fg-subtle)]', className)}
      htmlFor={htmlFor}
    >
      {label}
      {children}
    </label>
  );
}

export function ValueField({ children, className, label }: Omit<FieldProps, 'htmlFor'>) {
  return (
    <div className={classNames('grid min-w-0 content-start gap-1.5', className)}>
      <span className="txt-compact-xsmall text-[color:var(--fg-muted)]">{label}</span>
      <span className="txt-compact-small break-all text-[color:var(--fg-base)]">{children}</span>
    </div>
  );
}

interface CheckboxFieldProps extends Omit<InputHTMLAttributes<HTMLInputElement>, 'type'> {
  label: string;
}

export function CheckboxField({ className, label, ...props }: CheckboxFieldProps) {
  return (
    <label className={classNames('txt-compact-small flex min-h-8 items-center gap-2 text-[color:var(--fg-subtle)]', className)}>
      <input
        className="h-4 w-4 shrink-0 accent-zinc-900 dark:accent-zinc-100"
        type="checkbox"
        {...props}
      />
      <span>{label}</span>
    </label>
  );
}
