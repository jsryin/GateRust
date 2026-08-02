import { Pencil, Settings2 } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import { getProxyRuntime, setProxyListener } from '../lib/api';
import { errorMessage } from '../lib/errors';
import type { ProxyConfig, ProxyListenerConfig, ProxyRuntimeState } from '../lib/types';
import { AcmeAccountsPanel, DnsAccountsPanel } from './proxy/AccountPanels';
import { CertificatePanel } from './proxy/CertificatePanel';
import { RoutePanel } from './proxy/RoutePanel';
import { Button } from './ui/Button';
import { Dialog, DialogBody, DialogContent, DialogFooter } from './ui/Dialog';
import { Field, Input, ValueField } from './ui/Fields';
import { Notice } from './ui/Notice';
import { FormGrid, PageIntro } from './ui/Page';
import { Panel, PanelHeader } from './ui/Panel';

interface ProxyPanelProps {
  config: ProxyConfig | null | undefined;
  onSaved: (config: ProxyConfig) => void;
  token: string;
}

type Tab = 'certificates' | 'acme' | 'dns' | 'routes';

const defaultListener = (): ProxyListenerConfig => ({
  http_bind: '0.0.0.0:80',
  https_bind: '0.0.0.0:443',
  cache_dir: '/var/lib/gaterust/proxy/acme',
  max_connections: 2048
});

const defaultConfig = (): ProxyConfig => ({
  proxy: defaultListener(),
  acme_accounts: [],
  dns_accounts: [],
  certificates: [],
  routes: []
});

const tabs: { id: Tab; label: string }[] = [
  { id: 'certificates', label: '托管证书' },
  { id: 'acme', label: 'ACME 账户' },
  { id: 'dns', label: 'DNS 账户' },
  { id: 'routes', label: '域名路由' }
];

export function ProxyPanel({ config, onSaved, token }: ProxyPanelProps) {
  const [draft, setDraft] = useState<ProxyConfig>(() => structuredClone(config ?? defaultConfig()));
  const [tab, setTab] = useState<Tab>('certificates');
  const [listener, setListener] = useState<ProxyListenerConfig | null>(null);
  const [runtime, setRuntime] = useState<ProxyRuntimeState | null>(null);
  const [runtimeRevision, setRuntimeRevision] = useState(0);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');

  useEffect(() => setDraft(structuredClone(config ?? defaultConfig())), [config]);

  const refreshRuntime = useCallback(() => setRuntimeRevision((value) => value + 1), []);

  useEffect(() => {
    const controller = new AbortController();
    let timer: number | undefined;
    async function load() {
      try {
        setRuntime(await getProxyRuntime(token, controller.signal));
      } catch {
        // 短暂请求失败时保留最后一次状态，避免把申请中的证书误显示为未申请。
      }
      if (!controller.signal.aborted) timer = window.setTimeout(load, 3000);
    }
    void load();
    return () => {
      controller.abort();
      if (timer) window.clearTimeout(timer);
    };
  }, [runtimeRevision, token]);

  function saved(next: ProxyConfig) {
    setDraft(next);
    onSaved(next);
  }

  async function saveListener() {
    if (!listener || !listener.http_bind || !listener.https_bind || !listener.cache_dir || listener.max_connections < 1) {
      setError('监听地址、缓存目录不能为空，最大连接数必须大于 0');
      return;
    }
    setSaving(true);
    setError('');
    try {
      saved(await setProxyListener(token, listener));
      setListener(null);
      setMessage('代理监听配置已应用');
      refreshRuntime();
    } catch (cause) {
      setError(errorMessage(cause, '保存代理监听失败'));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="space-y-4">
      <PageIntro description="域名代理与证书账户" title="Web 与 SSL" />
      {message && <Notice tone="success">{message}</Notice>}
      {runtime?.config_status.last_apply_error && <Notice tone="error">{runtime.config_status.last_apply_error}</Notice>}

      <Panel>
        <PanelHeader action={<Button aria-label="编辑代理监听" onClick={() => { setListener({ ...draft.proxy }); setError(''); }} size="icon" variant="ghost"><Pencil className="h-4 w-4" /></Button>} title="代理监听" />
        <FormGrid columns={4}><ValueField label="HTTP 地址">{draft.proxy.http_bind}</ValueField><ValueField label="HTTPS 地址">{draft.proxy.https_bind}</ValueField><ValueField label="缓存目录">{draft.proxy.cache_dir}</ValueField><ValueField label="最大连接数">{draft.proxy.max_connections}</ValueField></FormGrid>
      </Panel>

      <nav aria-label="代理配置分类" className="flex min-h-10 gap-1 overflow-x-auto rounded-md bg-[var(--bg-component)] p-1 shadow-[var(--borders-base)]">
        {tabs.map((item) => <button aria-selected={tab === item.id} className={`transition-fg txt-compact-small-plus h-8 shrink-0 rounded-md px-3 ${tab === item.id ? 'bg-[var(--bg-base)] text-[color:var(--fg-base)] shadow-[var(--buttons-neutral)]' : 'text-[color:var(--fg-muted)] hover:text-[color:var(--fg-base)]'}`} key={item.id} onClick={() => setTab(item.id)} role="tab" type="button">{item.label}</button>)}
      </nav>

      {tab === 'certificates' && <CertificatePanel config={draft} onRuntimeRefresh={refreshRuntime} onSaved={saved} runtime={runtime?.certificates ?? []} token={token} />}
      {tab === 'acme' && <AcmeAccountsPanel config={draft} onSaved={saved} token={token} />}
      {tab === 'dns' && <DnsAccountsPanel config={draft} onSaved={saved} token={token} />}
      {tab === 'routes' && <RoutePanel config={draft} onSaved={saved} token={token} />}

      <Dialog open={listener !== null} onOpenChange={(value) => !value && !saving && setListener(null)}>
        {listener && <DialogContent title="代理监听"><DialogBody><div className="grid gap-4 sm:grid-cols-2"><Field label="HTTP 地址"><Input value={listener.http_bind} onChange={(event) => setListener({ ...listener, http_bind: event.target.value })} /></Field><Field label="HTTPS 地址"><Input value={listener.https_bind} onChange={(event) => setListener({ ...listener, https_bind: event.target.value })} /></Field><Field className="sm:col-span-2" label="缓存目录"><Input value={listener.cache_dir} onChange={(event) => setListener({ ...listener, cache_dir: event.target.value })} /></Field><Field label="最大连接数"><Input min="1" type="number" value={listener.max_connections} onChange={(event) => setListener({ ...listener, max_connections: Number(event.target.value) })} /></Field></div>{error && <p className="txt-compact-small mt-4 text-[color:var(--tag-red-text)]" role="alert">{error}</p>}</DialogBody><DialogFooter><Button disabled={saving} onClick={() => setListener(null)} variant="secondary">取消</Button><Button disabled={saving} onClick={() => void saveListener()}><Settings2 className="h-4 w-4" />{saving ? '保存中' : '保存'}</Button></DialogFooter></DialogContent>}
      </Dialog>
    </div>
  );
}
