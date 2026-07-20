import { Eye, EyeOff, FolderOpen, KeyRound, LoaderCircle } from 'lucide-react';
import { useState } from 'react';
import type { EditableClientConfig } from '../lib/types';

interface ConnectionSettingsProps {
  generating: boolean;
  groupKey: string;
  onChooseCertificate: () => Promise<void>;
  onGenerateKey: () => Promise<void>;
  onGroupKeyChange: (key: string) => void;
  onServerChange: (server: EditableClientConfig['server']) => void;
  server: EditableClientConfig['server'];
}

export function ConnectionSettings({
  generating,
  groupKey,
  onChooseCertificate,
  onGenerateKey,
  onGroupKeyChange,
  onServerChange,
  server
}: ConnectionSettingsProps) {
  const [keyVisible, setKeyVisible] = useState(false);

  return (
    <section aria-labelledby="connection-heading" className="settings-section">
      <header className="section-heading">
        <div>
          <h2 id="connection-heading">服务器连接</h2>
          <span>QUIC + TLS</span>
        </div>
      </header>

      <div className="connection-grid">
        <label>
          <span>服务器地址</span>
          <input
            onChange={(event) => onServerChange({ ...server, address: event.target.value })}
            placeholder="tunnel.example.com:2333"
            required
            spellCheck={false}
            value={server.address}
          />
        </label>

        <label>
          <span>TLS 服务器名称</span>
          <input
            onChange={(event) => onServerChange({ ...server, name: event.target.value })}
            placeholder="tunnel.example.com"
            spellCheck={false}
            value={server.name}
          />
        </label>

        <label className="wide-field">
          <span>CA 证书</span>
          <span className="field-action">
            <input
              onChange={(event) => onServerChange({ ...server, caCertificate: event.target.value })}
              placeholder="使用系统受信 CA"
              spellCheck={false}
              value={server.caCertificate}
            />
            <button
              aria-label="选择证书文件"
              className="icon-button attached"
              onClick={() => void onChooseCertificate()}
              title="选择证书文件"
              type="button"
            >
              <FolderOpen aria-hidden="true" size={17} strokeWidth={1.8} />
            </button>
          </span>
        </label>

        <label className="wide-field">
          <span>分组密钥</span>
          <span className="field-action key-action">
            <input
              autoComplete="off"
              maxLength={124}
              minLength={32}
              onChange={(event) => onGroupKeyChange(event.target.value)}
              required
              spellCheck={false}
              type={keyVisible ? 'text' : 'password'}
              value={groupKey}
            />
            <button
              aria-label={keyVisible ? '隐藏密钥' : '显示密钥'}
              className="icon-button attached"
              onClick={() => setKeyVisible((visible) => !visible)}
              title={keyVisible ? '隐藏密钥' : '显示密钥'}
              type="button"
            >
              {keyVisible ? (
                <EyeOff aria-hidden="true" size={17} strokeWidth={1.8} />
              ) : (
                <Eye aria-hidden="true" size={17} strokeWidth={1.8} />
              )}
            </button>
            <button
              className="secondary-button generate-button"
              disabled={generating}
              onClick={() => void onGenerateKey()}
              title="生成新密钥"
              type="button"
            >
              {generating ? (
                <LoaderCircle aria-hidden="true" className="spin" size={16} />
              ) : (
                <KeyRound aria-hidden="true" size={16} />
              )}
              生成
            </button>
          </span>
        </label>
      </div>
    </section>
  );
}
