import { Globe2, Pencil, Plus, Save, ShieldCheck, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { saveProxy } from '../lib/api';
import { errorMessage } from '../lib/errors';
import type {
  AcmeChallenge,
  CertificateConfig,
  CertificateIssuer,
  ProxyConfig,
  RouteConfig
} from '../lib/types';
import { Badge } from './ui/Badge';
import { Button } from './ui/Button';
import { Dialog, DialogBody, DialogContent, DialogFooter } from './ui/Dialog';
import { CheckboxField, Field, Input, Select, Textarea } from './ui/Fields';
import { FormGrid, PageIntro } from './ui/Page';
import { EmptyState, Panel, PanelHeader } from './ui/Panel';
import { Notice } from './ui/Notice';
import { Table, TableCell, TableHead, TableHeader, TableRow } from './ui/Table';

interface ProxyPanelProps {
  config: ProxyConfig | null | undefined;
  onSaved: (config: ProxyConfig) => void;
  token: string;
}

type Tab = 'certificates' | 'routes';
type Editor = 'certificate' | 'route' | null;

function defaultCertificate(): CertificateConfig {
  return {
    name: '',
    domains: [],
    email: '',
    issuer: 'lets_encrypt',
    challenge: 'tls-alpn-01',
    production: false,
    cloudflare_api_token: null,
    cloudflare_zone_id: null,
    google_eab_key_id: null,
    google_eab_hmac_key: null,
    dns_propagation_seconds: 30
  };
}

function defaultRoute(): RouteConfig {
  return {
    name: '',
    host: '',
    path_prefix: '/',
    upstream: 'http://127.0.0.1:3000',
    certificate: null
  };
}

function defaultConfig(): ProxyConfig {
  return {
    proxy: {
      http_bind: '0.0.0.0:80',
      https_bind: '0.0.0.0:443',
      cache_dir: '/var/lib/gaterust/proxy/acme',
      max_connections: 2048
    },
    certificates: [],
    routes: []
  };
}

export function ProxyPanel({ config, onSaved, token }: ProxyPanelProps) {
  const [draft, setDraft] = useState<ProxyConfig>(() => structuredClone(config ?? defaultConfig()));
  const [tab, setTab] = useState<Tab>('certificates');
  const [editor, setEditor] = useState<Editor>(null);
  const [editIndex, setEditIndex] = useState(-1);
  const [certificate, setCertificate] = useState<CertificateConfig>(defaultCertificate);
  const [route, setRoute] = useState<RouteConfig>(defaultRoute);
  const [domains, setDomains] = useState('');
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');

  function openCertificate(index = -1) {
    const next = index >= 0 ? structuredClone(draft.certificates[index]) : defaultCertificate();
    setEditIndex(index);
    setCertificate(next);
    setDomains(next.domains.join('\n'));
    setEditor('certificate');
    setError('');
  }

  function openRoute(index = -1) {
    setEditIndex(index);
    setRoute(index >= 0 ? { ...draft.routes[index] } : defaultRoute());
    setEditor('route');
    setError('');
  }

  function commitCertificate() {
    const next: CertificateConfig = {
      ...certificate,
      domains: domains.split(/[\s,]+/).filter(Boolean),
      cloudflare_api_token: certificate.challenge === 'cloudflare-dns-01' ? certificate.cloudflare_api_token : null,
      cloudflare_zone_id: certificate.challenge === 'cloudflare-dns-01' ? certificate.cloudflare_zone_id : null,
      google_eab_key_id: certificate.issuer === 'google_trust_services' ? certificate.google_eab_key_id : null,
      google_eab_hmac_key: certificate.issuer === 'google_trust_services' ? certificate.google_eab_hmac_key : null
    };

    if (!next.name || !next.email || !next.domains.length) {
      setError('名称、域名和联系邮箱不能为空');
      return;
    }

    setDraft((current) => {
      const certificates = [...current.certificates];
      if (editIndex >= 0) certificates[editIndex] = next;
      else certificates.push(next);
      return { ...current, certificates };
    });
    setEditor(null);
  }

  function commitRoute() {
    if (!route.name || !route.host || !route.upstream) {
      setError('名称、域名和上游地址不能为空');
      return;
    }

    setDraft((current) => {
      const routes = [...current.routes];
      if (editIndex >= 0) routes[editIndex] = route;
      else routes.push(route);
      return { ...current, routes };
    });
    setEditor(null);
  }

  function removeCertificate(index: number) {
    const name = draft.certificates[index].name;
    setDraft((current) => ({
      ...current,
      certificates: current.certificates.filter((_, currentIndex) => currentIndex !== index),
      routes: current.routes.map((item) => item.certificate === name ? { ...item, certificate: null } : item)
    }));
  }

  function removeRoute(index: number) {
    setDraft((current) => ({
      ...current,
      routes: current.routes.filter((_, currentIndex) => currentIndex !== index)
    }));
  }

  async function persist() {
    setSaving(true);
    setMessage('');
    setError('');
    try {
      const saved = await saveProxy(token, draft);
      setDraft(saved);
      onSaved(saved);
      setMessage('配置已保存；首次启用或修改监听参数时请重启服务');
    } catch (cause) {
      setError(errorMessage(cause, '保存失败'));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="space-y-4">
      <PageIntro
        action={(
          <Button disabled={saving} onClick={() => void persist()}>
            <Save className="h-4 w-4" />
            {saving ? '保存中' : '保存配置'}
          </Button>
        )}
        description="管理反向代理入口、ACME 证书与域名路由"
        title="代理配置"
      />
      {message && <Notice tone="success">{message}</Notice>}
      {error && !editor && <Notice tone="error">{error}</Notice>}

      <Panel>
        <PanelHeader description="HTTP、HTTPS 入口与 ACME 缓存" title="代理监听" />
        <FormGrid columns={4}>
          <Field label="HTTP 地址">
            <Input onChange={(event) => setDraft((current) => ({ ...current, proxy: { ...current.proxy, http_bind: event.target.value } }))} value={draft.proxy.http_bind} />
          </Field>
          <Field label="HTTPS 地址">
            <Input onChange={(event) => setDraft((current) => ({ ...current, proxy: { ...current.proxy, https_bind: event.target.value } }))} value={draft.proxy.https_bind} />
          </Field>
          <Field label="缓存目录">
            <Input onChange={(event) => setDraft((current) => ({ ...current, proxy: { ...current.proxy, cache_dir: event.target.value } }))} value={draft.proxy.cache_dir} />
          </Field>
          <Field label="最大连接数">
            <Input min="1" onChange={(event) => setDraft((current) => ({ ...current, proxy: { ...current.proxy, max_connections: Number(event.target.value) } }))} type="number" value={draft.proxy.max_connections} />
          </Field>
        </FormGrid>
      </Panel>

      <div className="inline-flex rounded-md bg-[var(--bg-component)] p-0.5 shadow-[var(--borders-base)]" role="tablist">
        <button
          aria-selected={tab === 'certificates'}
          className={`transition-fg txt-compact-small-plus h-7 rounded px-3 ${tab === 'certificates' ? 'bg-[var(--bg-base)] text-[color:var(--fg-base)] shadow-[var(--buttons-neutral)]' : 'text-[color:var(--fg-muted)] hover:text-[color:var(--fg-base)]'}`}
          onClick={() => setTab('certificates')}
          role="tab"
          type="button"
        >
          证书
        </button>
        <button
          aria-selected={tab === 'routes'}
          className={`transition-fg txt-compact-small-plus h-7 rounded px-3 ${tab === 'routes' ? 'bg-[var(--bg-base)] text-[color:var(--fg-base)] shadow-[var(--buttons-neutral)]' : 'text-[color:var(--fg-muted)] hover:text-[color:var(--fg-base)]'}`}
          onClick={() => setTab('routes')}
          role="tab"
          type="button"
        >
          域名路由
        </button>
      </div>

      {tab === 'certificates' ? (
        <Panel>
          <PanelHeader
            action={(
              <Button onClick={() => openCertificate()} variant="secondary">
                <Plus className="h-4 w-4" />
                新建证书
              </Button>
            )}
            description="ACME 签发、DNS Provider 与续期配置"
            title="托管证书"
          />
          {draft.certificates.length ? (
            <Table className="min-w-[860px]">
              <TableHeader>
                <TableRow>
                  <TableHead>名称</TableHead>
                  <TableHead>域名</TableHead>
                  <TableHead>签发机构</TableHead>
                  <TableHead>验证方式</TableHead>
                  <TableHead>环境</TableHead>
                  <TableHead className="text-right">操作</TableHead>
                </TableRow>
              </TableHeader>
              <tbody>
                {draft.certificates.map((item, index) => (
                  <TableRow key={`${item.name}-${index}`}>
                    <TableCell className="font-medium text-[color:var(--fg-base)]">{item.name}</TableCell>
                    <TableCell className="max-w-72 truncate">{item.domains.join(', ')}</TableCell>
                    <TableCell>{item.issuer === 'lets_encrypt' ? "Let's Encrypt" : 'Google Trust Services'}</TableCell>
                    <TableCell><code className="text-xs">{item.challenge}</code></TableCell>
                    <TableCell><Badge tone={item.production ? 'green' : 'orange'}>{item.production ? '生产' : '测试'}</Badge></TableCell>
                    <TableCell>
                      <div className="flex justify-end gap-1">
                        <Button aria-label={`编辑 ${item.name}`} onClick={() => openCertificate(index)} size="icon" variant="ghost"><Pencil className="h-4 w-4" /></Button>
                        <Button aria-label={`删除 ${item.name}`} onClick={() => removeCertificate(index)} size="icon" variant="ghost"><Trash2 className="h-4 w-4 text-[color:var(--tag-red-text)]" /></Button>
                      </div>
                    </TableCell>
                  </TableRow>
                ))}
              </tbody>
            </Table>
          ) : (
            <EmptyState description="添加证书后可在域名路由中直接绑定。" icon={ShieldCheck} title="还没有托管证书" />
          )}
        </Panel>
      ) : (
        <Panel>
          <PanelHeader
            action={(
              <Button onClick={() => openRoute()} variant="secondary">
                <Plus className="h-4 w-4" />
                新建路由
              </Button>
            )}
            description="按 Host 和路径前缀转发至上游"
            title="域名路由"
          />
          {draft.routes.length ? (
            <Table className="min-w-[760px]">
              <TableHeader>
                <TableRow>
                  <TableHead>名称</TableHead>
                  <TableHead>Host / Path</TableHead>
                  <TableHead>上游</TableHead>
                  <TableHead>SSL 证书</TableHead>
                  <TableHead className="text-right">操作</TableHead>
                </TableRow>
              </TableHeader>
              <tbody>
                {draft.routes.map((item, index) => (
                  <TableRow key={`${item.name}-${index}`}>
                    <TableCell className="font-medium text-[color:var(--fg-base)]">{item.name}</TableCell>
                    <TableCell><code className="text-xs">{item.host}{item.path_prefix}</code></TableCell>
                    <TableCell>{item.upstream}</TableCell>
                    <TableCell>{item.certificate ?? '不启用'}</TableCell>
                    <TableCell>
                      <div className="flex justify-end gap-1">
                        <Button aria-label={`编辑 ${item.name}`} onClick={() => openRoute(index)} size="icon" variant="ghost"><Pencil className="h-4 w-4" /></Button>
                        <Button aria-label={`删除 ${item.name}`} onClick={() => removeRoute(index)} size="icon" variant="ghost"><Trash2 className="h-4 w-4 text-[color:var(--tag-red-text)]" /></Button>
                      </div>
                    </TableCell>
                  </TableRow>
                ))}
              </tbody>
            </Table>
          ) : (
            <EmptyState description="路由可指向本地隧道端口或公网 HTTP(S) 上游。" icon={Globe2} title="还没有域名路由" />
          )}
        </Panel>
      )}

      <Dialog open={editor !== null} onOpenChange={(open) => !open && setEditor(null)}>
        {editor && (
          <DialogContent
            description={editIndex >= 0 ? '修改现有配置项' : '创建新的配置项'}
            title={editor === 'certificate' ? '托管证书' : '域名路由'}
          >
            <DialogBody>
              <div className="grid gap-4 sm:grid-cols-2">
                {editor === 'certificate' ? (
                  <>
                    <Field label="名称"><Input onChange={(event) => setCertificate((current) => ({ ...current, name: event.target.value }))} value={certificate.name} /></Field>
                    <Field label="联系邮箱"><Input onChange={(event) => setCertificate((current) => ({ ...current, email: event.target.value }))} type="email" value={certificate.email} /></Field>
                    <Field className="sm:col-span-2" label="域名（每行一个）"><Textarea onChange={(event) => setDomains(event.target.value)} rows={3} value={domains} /></Field>
                    <Field label="签发机构">
                      <Select onChange={(event) => setCertificate((current) => ({ ...current, issuer: event.target.value as CertificateIssuer }))} value={certificate.issuer}>
                        <option value="lets_encrypt">Let's Encrypt</option>
                        <option value="google_trust_services">Google Trust Services</option>
                      </Select>
                    </Field>
                    <Field label="验证方式">
                      <Select onChange={(event) => setCertificate((current) => ({ ...current, challenge: event.target.value as AcmeChallenge }))} value={certificate.challenge}>
                        <option value="http-01">HTTP-01</option>
                        <option value="tls-alpn-01">TLS-ALPN-01</option>
                        <option value="cloudflare-dns-01">Cloudflare DNS-01</option>
                      </Select>
                    </Field>
                    <CheckboxField checked={certificate.production} className="sm:col-span-2" label="使用生产环境证书" onChange={(event) => setCertificate((current) => ({ ...current, production: event.target.checked }))} />
                    {certificate.challenge === 'cloudflare-dns-01' && (
                      <>
                        <Field label="Cloudflare API Token"><Input onChange={(event) => setCertificate((current) => ({ ...current, cloudflare_api_token: event.target.value }))} type="password" value={certificate.cloudflare_api_token ?? ''} /></Field>
                        <Field label="Cloudflare Zone ID"><Input onChange={(event) => setCertificate((current) => ({ ...current, cloudflare_zone_id: event.target.value }))} value={certificate.cloudflare_zone_id ?? ''} /></Field>
                        <Field label="DNS 传播秒数"><Input max="600" min="1" onChange={(event) => setCertificate((current) => ({ ...current, dns_propagation_seconds: Number(event.target.value) }))} type="number" value={certificate.dns_propagation_seconds} /></Field>
                      </>
                    )}
                    {certificate.issuer === 'google_trust_services' && (
                      <>
                        <Field label="Google EAB Key ID"><Input onChange={(event) => setCertificate((current) => ({ ...current, google_eab_key_id: event.target.value }))} value={certificate.google_eab_key_id ?? ''} /></Field>
                        <Field label="Google EAB HMAC Key"><Input onChange={(event) => setCertificate((current) => ({ ...current, google_eab_hmac_key: event.target.value }))} type="password" value={certificate.google_eab_hmac_key ?? ''} /></Field>
                      </>
                    )}
                  </>
                ) : (
                  <>
                    <Field label="名称"><Input onChange={(event) => setRoute((current) => ({ ...current, name: event.target.value }))} value={route.name} /></Field>
                    <Field label="域名"><Input onChange={(event) => setRoute((current) => ({ ...current, host: event.target.value }))} placeholder="example.com" value={route.host} /></Field>
                    <Field label="路径前缀"><Input onChange={(event) => setRoute((current) => ({ ...current, path_prefix: event.target.value }))} value={route.path_prefix} /></Field>
                    <Field label="上游地址"><Input onChange={(event) => setRoute((current) => ({ ...current, upstream: event.target.value }))} value={route.upstream} /></Field>
                    <Field label="SSL 证书">
                      <Select onChange={(event) => setRoute((current) => ({ ...current, certificate: event.target.value || null }))} value={route.certificate ?? ''}>
                        <option value="">不启用</option>
                        {draft.certificates.map((item) => <option key={item.name} value={item.name}>{item.name}</option>)}
                      </Select>
                    </Field>
                  </>
                )}
              </div>
              {error && <p className="txt-compact-small mt-4 text-[color:var(--tag-red-text)]" role="alert">{error}</p>}
            </DialogBody>
            <DialogFooter>
              <Button onClick={() => setEditor(null)} variant="secondary">取消</Button>
              <Button onClick={editor === 'certificate' ? commitCertificate : commitRoute}>确认</Button>
            </DialogFooter>
          </DialogContent>
        )}
      </Dialog>
    </div>
  );
}
