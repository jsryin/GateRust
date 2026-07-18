import { forwardRef, type ButtonHTMLAttributes } from 'react';
import { classNames } from '../../lib/class-names';

type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger';
type ButtonSize = 'default' | 'small' | 'icon';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
}

const variants: Record<ButtonVariant, string> = {
  primary:
    'bg-[var(--button-inverted)] text-[color:var(--contrast-fg-primary)] shadow-[var(--buttons-inverted)] hover:bg-[var(--button-inverted-hover)] active:bg-[var(--button-inverted-pressed)] focus-visible:shadow-[var(--buttons-inverted-focus)]',
  secondary:
    'bg-[var(--button-neutral)] text-[color:var(--fg-base)] shadow-[var(--buttons-neutral)] hover:bg-[var(--button-neutral-hover)] active:bg-[var(--button-neutral-pressed)] focus-visible:shadow-[var(--buttons-neutral-focus)]',
  ghost:
    'bg-[var(--button-transparent)] text-[color:var(--fg-subtle)] hover:bg-[var(--button-transparent-hover)] hover:text-[color:var(--fg-base)] active:bg-[var(--button-transparent-pressed)] focus-visible:bg-[var(--bg-base)] focus-visible:shadow-[var(--buttons-neutral-focus)]',
  danger:
    'bg-[var(--button-danger)] text-[color:var(--fg-on-color)] shadow-[var(--buttons-danger)] hover:bg-[var(--button-danger-hover)] active:bg-[var(--button-danger-pressed)] focus-visible:shadow-[var(--buttons-danger-focus)]'
};

const sizes: Record<ButtonSize, string> = {
  default: 'h-8 px-3',
  small: 'h-7 px-2',
  icon: 'h-7 w-7 p-1'
};

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant = 'primary', size = 'default', type = 'button', ...props }, ref) => (
    <button
      ref={ref}
      className={classNames(
        'transition-fg txt-compact-small-plus relative inline-flex shrink-0 items-center justify-center gap-1.5 overflow-hidden whitespace-nowrap rounded-md outline-none disabled:pointer-events-none disabled:bg-[var(--bg-disabled)] disabled:text-[color:var(--fg-disabled)] disabled:shadow-[var(--buttons-neutral)]',
        variants[variant],
        sizes[size],
        className
      )}
      type={type}
      {...props}
    />
  )
);

Button.displayName = 'Button';
