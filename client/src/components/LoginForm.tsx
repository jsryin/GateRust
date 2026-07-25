import { ArrowLeft, Eye, EyeOff, LoaderCircle, LogIn, X } from 'lucide-react';
import { useState } from 'react';
import type { FormEvent } from 'react';

type LoginFormState = 'idle' | 'fetching' | 'cancelling' | 'connecting';

interface LoginFormProps {
  address: string;
  error: string;
  keyValue: string;
  onAddressChange: (value: string) => void;
  onBack?: () => void;
  onCancel: () => Promise<void>;
  onKeyChange: (value: string) => void;
  onSubmit: () => Promise<void>;
  state: LoginFormState;
}

export function LoginForm({
  address,
  error,
  keyValue,
  onAddressChange,
  onBack,
  onCancel,
  onKeyChange,
  onSubmit,
  state
}: LoginFormProps) {
  const [keyVisible, setKeyVisible] = useState(false);
  const fetching = state === 'fetching' || state === 'cancelling';
  const pending = state !== 'idle';
  const primaryLabel = state === 'idle' ? '获取配置' : state === 'connecting' ? '连接中' : '获取中';

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    void onSubmit();
  }

  return (
    <form className="login-form" onSubmit={submit}>
      <header>
        {onBack && (
          <button
            aria-label="返回隧道列表"
            className="login-back-button"
            disabled={pending}
            onClick={onBack}
            title="返回隧道列表"
            type="button"
          >
            <ArrowLeft size={17} />
          </button>
        )}
        <h1>获取连接配置</h1>
      </header>
      <label>
        <span>服务器地址</span>
        <input
          autoFocus
          disabled={pending}
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
            disabled={pending}
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
      <div className="login-actions">
        <button className="primary-button" disabled={pending} type="submit">
          {pending ? <LoaderCircle className="spin" size={16} /> : <LogIn size={16} />}
          {primaryLabel}
        </button>
        {fetching && (
          <button
            className="secondary-button"
            disabled={state === 'cancelling'}
            onClick={() => void onCancel()}
            type="button"
          >
            {state === 'cancelling' ? <LoaderCircle className="spin" size={16} /> : <X size={16} />}
            {state === 'cancelling' ? '取消中' : '取消获取'}
          </button>
        )}
      </div>
    </form>
  );
}
