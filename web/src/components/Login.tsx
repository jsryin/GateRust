import { useState, type FormEvent } from 'react';
import type { Theme } from '../hooks/useTheme';
import { login } from '../lib/api';
import { errorMessage } from '../lib/errors';
import { ThemeButton } from './ThemeButton';
import { Button } from './ui/Button';
import { Field, Input } from './ui/Fields';

interface LoginProps {
  onAuthenticated: (token: string) => void;
  onToggleTheme: () => void;
  theme: Theme;
}

export function Login({ onAuthenticated, onToggleTheme, theme }: LoginProps) {
  const [username, setUsername] = useState('admin');
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    setError('');
    try {
      const session = await login(username, password);
      onAuthenticated(session.token);
    } catch (cause) {
      setError(errorMessage(cause, '登录失败'));
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="relative grid min-h-screen place-items-center bg-[var(--bg-subtle)] px-4 py-10">
      <div className="absolute right-4 top-4">
        <ThemeButton onToggle={onToggleTheme} theme={theme} />
      </div>
      <section className="w-full max-w-[380px] overflow-hidden rounded-lg bg-[var(--bg-base)] shadow-[var(--elevation-card-rest)]">
        <div className="px-6 py-6">
          <h1 className="mb-5 border-b border-[color:var(--border-base)] pb-5 text-center txt-compact-large-plus">登录</h1>
          <form className="grid gap-4" onSubmit={submit}>
            <Field htmlFor="username" label="用户名">
              <Input
                autoComplete="username"
                id="username"
                onChange={(event) => setUsername(event.target.value)}
                required
                value={username}
              />
            </Field>
            <Field htmlFor="password" label="密码">
              <Input
                autoComplete="current-password"
                autoFocus
                id="password"
                onChange={(event) => setPassword(event.target.value)}
                required
                type="password"
                value={password}
              />
            </Field>
            {error && <p className="txt-compact-small text-[color:var(--tag-red-text)]" role="alert">{error}</p>}
            <Button className="mt-1 w-full" disabled={busy} type="submit">
              {busy ? '正在验证' : '登录'}
            </Button>
          </form>
        </div>
      </section>
    </main>
  );
}
