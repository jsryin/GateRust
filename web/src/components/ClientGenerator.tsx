import { Check, Clipboard, Download, FileCode2, WandSparkles } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { generateClientConfig } from '../lib/api';
import { errorMessage } from '../lib/errors';
import type { ClientService, ServerConfig } from '../lib/types';
import { Badge } from './ui/Badge';
import { Button } from './ui/Button';
import { CheckboxField, Field, Input, Select } from './ui/Fields';
import { FormGrid, PageIntro } from './ui/Page';
import { EmptyState, Panel, PanelHeader } from './ui/Panel';
import { Notice } from './ui/Notice';

interface ClientGeneratorProps {
  config: ServerConfig | null | undefined;
  token: string;
}

export function ClientGenerator({ config, token }: ClientGeneratorProps) {
  const [group, setGroup] = useState(config?.groups[0]?.name ?? '');
  const [serverAddress, setServerAddress] = useState(
    config?.quic.bind.replace('0.0.0.0', 'server.example.com') ?? 'server.example.com:2333'
  );
  const [serverName, setServerName] = useState('');
  const [caCertificate, setCaCertificate] = useState('');
  const [services, setServices] = useState<ClientService[]>(
    () => config?.tunnels.map((tunnel) => ({
      name: tunnel.name,
      kind: tunnel.kind,
      target: tunnel.kind === 'socks5' ? null : '127.0.0.1:8080'
    })) ?? []
  );
  const [result, setResult] = useState('');
  const [error, setError] = useState('');
  const [copied, setCopied] = useState(false);
  const [generating, setGenerating] = useState(false);
  const copiedTimer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(copiedTimer.current), []);

  async function generate() {
    setError('');
    setGenerating(true);
    try {
      const response = await generateClientConfig(token, {
        group,
        server_address: serverAddress,
        server_name: serverName || null,
        ca_certificate: caCertificate || null,
        services
      });
      setResult(response.toml);
    } catch (cause) {
      setError(errorMessage(cause, '生成失败'));
    } finally {
      setGenerating(false);
    }
  }

  async function copy() {
    try {
      await navigator.clipboard.writeText(result);
      setCopied(true);
      window.clearTimeout(copiedTimer.current);
      copiedTimer.current = window.setTimeout(() => setCopied(false), 1800);
    } catch (cause) {
      setError(errorMessage(cause, '复制失败'));
    }
  }

  function download() {
    const url = URL.createObjectURL(new Blob([result], { type: 'text/plain' }));
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = 'client.toml';
    anchor.click();
    URL.revokeObjectURL(url);
  }

  function updateTarget(index: number, target: string) {
    setServices((current) => current.map((service, currentIndex) => (
      currentIndex === index ? { ...service, target } : service
    )));
  }

  function removeService(index: number) {
    setServices((current) => current.filter((_, currentIndex) => currentIndex !== index));
  }

  return (
    <div className="space-y-4">
      <PageIntro description="生成包含认证分组和本地服务映射的客户端配置" title="客户端配置" />
      {!config || !config.groups.length ? (
        <Panel>
          <EmptyState description="客户端配置包含所选分组的认证密钥。" icon={FileCode2} title="需要先创建访问分组" />
        </Panel>
      ) : (
        <>
          {error && <Notice tone="error">{error}</Notice>}
          <Panel>
            <PanelHeader description="用于建立 QUIC 控制连接" title="连接信息" />
            <FormGrid columns={4}>
              <Field label="访问分组">
                <Select onChange={(event) => setGroup(event.target.value)} value={group}>
                  {config.groups.map((item) => <option key={item.name} value={item.name}>{item.name}</option>)}
                </Select>
              </Field>
              <Field label="服务器地址"><Input onChange={(event) => setServerAddress(event.target.value)} value={serverAddress} /></Field>
              <Field label="TLS 服务器名称（可选）"><Input onChange={(event) => setServerName(event.target.value)} value={serverName} /></Field>
              <Field label="CA 证书路径（可选）"><Input onChange={(event) => setCaCertificate(event.target.value)} value={caCertificate} /></Field>
            </FormGrid>
          </Panel>

          <Panel>
            <PanelHeader description="为服务端隧道指定此客户端上的目标地址" title="本地服务" />
            {services.length ? (
              <div className="px-5 sm:px-6">
                {services.map((service, index) => (
                  <div className="grid gap-3 border-b border-[color:var(--border-base)] py-4 last:border-b-0 sm:grid-cols-[minmax(140px,1fr)_minmax(220px,2fr)_80px] sm:items-center" key={`${service.name}-${index}`}>
                    <div className="flex min-w-0 items-center gap-2">
                      <strong className="txt-compact-small-plus truncate font-medium">{service.name}</strong>
                      <Badge>{service.kind.toUpperCase()}</Badge>
                    </div>
                    {service.kind === 'socks5' ? (
                      <p className="txt-compact-small text-[color:var(--fg-muted)]">按请求动态连接目标</p>
                    ) : (
                      <Field label="目标地址">
                        <Input onChange={(event) => updateTarget(index, event.target.value)} placeholder="127.0.0.1:8080" value={service.target ?? ''} />
                      </Field>
                    )}
                    <CheckboxField checked label="包含" onChange={(event) => !event.target.checked && removeService(index)} />
                  </div>
                ))}
              </div>
            ) : (
              <EmptyState title="未选择本地服务" />
            )}
            <div className="flex justify-end border-t border-[color:var(--border-base)] px-5 py-4 sm:px-6">
              <Button disabled={generating} onClick={() => void generate()}>
                <WandSparkles className="h-4 w-4" />
                {generating ? '生成中' : '生成配置'}
              </Button>
            </div>
          </Panel>

          {result && (
            <Panel>
              <PanelHeader
                action={(
                  <div className="flex gap-2">
                    <Button onClick={() => void copy()} variant="secondary">
                      {copied ? <Check className="h-4 w-4" /> : <Clipboard className="h-4 w-4" />}
                      {copied ? '已复制' : '复制'}
                    </Button>
                    <Button onClick={download} variant="secondary">
                      <Download className="h-4 w-4" />
                      下载
                    </Button>
                  </div>
                )}
                description="密钥仅展示在当前登录会话中"
                title="client.toml"
              />
              <pre className="max-h-[420px] overflow-auto bg-zinc-950 p-5 text-xs leading-6 text-zinc-200">{result}</pre>
            </Panel>
          )}
        </>
      )}
    </div>
  );
}
