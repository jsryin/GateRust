import { LockKeyhole, LogIn, Network } from 'lucide-react';
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
        <div className="border-b border-[color:var(--border-base)] px-6 py-5">
          <div className="flex items-center gap-3">
            <div className="grid h-9 w-9 place-items-center rounded-md bg-[var(--button-inverted)] text-[color:var(--contrast-fg-primary)] shadow-[var(--buttons-inverted)]">
              <Network className="h-5 w-5" />
            </div>
            <div>
              <h1 className="txt-compact-large-plus">GateRust</h1>
              <p className="txt-compact-xsmall text-[color:var(--fg-muted)]">中心控制台</p>
            </div>
          </div>
        </div>
        <div className="px-6 py-6">
          <div className="mb-5">
            <div className="mb-2 flex items-center gap-2 text-[color:var(--fg-subtle)]">
              <LockKeyhole className="h-4 w-4" />
              <span className="txt-compact-small-plus">管理员登录</span>
            </div>
            <p className="txt-compact-small text-[color:var(--fg-muted)]">使用管理员凭据继续</p>
          </div>
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
              <LogIn className="h-4 w-4" />
              {busy ? '正在验证' : '登录'}
            </Button>
          </form>
        </div>
      </section>
    </main>
  );
}
