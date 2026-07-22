import { Eye, EyeOff, LoaderCircle, LogIn } from 'lucide-react';
import { useState } from 'react';
import type { FormEvent } from 'react';

interface LoginFormProps {
  address: string;
  error: string;
  keyValue: string;
  onAddressChange: (value: string) => void;
  onKeyChange: (value: string) => void;
  onSubmit: () => Promise<void>;
  pending: boolean;
}

export function LoginForm({
  address,
  error,
  keyValue,
  onAddressChange,
  onKeyChange,
  onSubmit,
  pending
}: LoginFormProps) {
  const [keyVisible, setKeyVisible] = useState(false);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    void onSubmit();
  }

  return (
    <form className="login-form" onSubmit={submit}>
      <header>
        <h1>登录服务器</h1>
      </header>
      <label>
        <span>服务器地址</span>
        <input
          autoFocus
          onChange={(event) => onAddressChange(event.target.value)}
          placeholder="tunnel.example.com:2333"
          required
          spellCheck={false}
          value={address}
        />
      </label>
      <label>
        <span>分组密钥</span>
        <span className="key-field">
          <input
            autoComplete="off"
            maxLength={124}
            minLength={32}
            onChange={(event) => onKeyChange(event.target.value)}
            required
            spellCheck={false}
            type={keyVisible ? 'text' : 'password'}
            value={keyValue}
          />
          <button
            aria-label={keyVisible ? '隐藏密钥' : '显示密钥'}
            className="field-icon-button"
            onClick={() => setKeyVisible((visible) => !visible)}
            title={keyVisible ? '隐藏密钥' : '显示密钥'}
            type="button"
          >
            {keyVisible ? <EyeOff size={17} /> : <Eye size={17} />}
          </button>
        </span>
      </label>
      {error && <div className="notice error" role="alert">{error}</div>}
      <button className="primary-button login-button" disabled={pending} type="submit">
        {pending ? <LoaderCircle className="spin" size={16} /> : <LogIn size={16} />}
        {pending ? '登录中' : '登录'}
      </button>
    </form>
  );
}
