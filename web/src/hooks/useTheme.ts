import { useEffect, useState } from 'react';

export type Theme = 'light' | 'dark';

function initialTheme(): Theme {
  const stored = localStorage.getItem('gaterust_theme');
  if (stored === 'light' || stored === 'dark') return stored;
  return matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

export function useTheme() {
  const [theme, setTheme] = useState<Theme>(initialTheme);

  useEffect(() => {
    document.documentElement.classList.toggle('dark', theme === 'dark');
    localStorage.setItem('gaterust_theme', theme);
  }, [theme]);

  return {
    theme,
    toggleTheme: () => setTheme((current) => (current === 'dark' ? 'light' : 'dark'))
  };
}
