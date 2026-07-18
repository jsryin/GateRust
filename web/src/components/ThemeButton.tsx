import { Moon, Sun } from 'lucide-react';
import type { Theme } from '../hooks/useTheme';
import { Button } from './ui/Button';

interface ThemeButtonProps {
  theme: Theme;
  onToggle: () => void;
}

export function ThemeButton({ onToggle, theme }: ThemeButtonProps) {
  return (
    <Button aria-label={theme === 'dark' ? '切换到浅色模式' : '切换到深色模式'} onClick={onToggle} size="icon" variant="secondary">
      {theme === 'dark' ? <Sun className="h-4 w-4" /> : <Moon className="h-4 w-4" />}
    </Button>
  );
}
