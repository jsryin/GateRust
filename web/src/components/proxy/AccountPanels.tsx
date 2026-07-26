import { KeyRound, Pencil, Plus, ServerCog, TestTube2, Trash2 } from 'lucide-react';
import { useState } from 'react';
import {
  createAcmeAccount,
  createDnsAccount,
  deleteAcmeAccount,
  deleteDnsAccount,
  testDnsAccount,
  updateAcmeAccount,
  updateDnsAccount
} from '../../lib/api';
import { errorMessage } from '../../lib/errors';
import type {
  AcmeAccountInput,
  AcmeAccountView,
  DnsAccountInput,
  DnsAccountView,
  DnsProvider,
  ProxyConfig
} from '../../lib/types';
import { Button } from '../ui/Button';
import { ConfirmAction } from '../ui/ConfirmAction';
import { Dialog, DialogBody, DialogContent, DialogFooter } from '../ui/Dialog';
import { Field, Input, Select } from '../ui/Fields';
import { Notice } from '../ui/Notice';
import { EmptyState, Panel, PanelHeader } from '../ui/Panel';
import { Table, TableCell, TableHead, TableHeader, TableRow } from '../ui/Table';

interface AccountPanelProps {
  config: ProxyConfig;
  onSaved: (config: ProxyConfig) => void;
  token: string;
}

const acmeProviderLabels = {
  lets_encrypt: "Let's Encrypt",
  google_cloud: 'Google Cloud Public CA'
} as const;

const dnsProviderLabels: Record<DnsProvider, string> = {
  cloudflare: 'Cloudflare',
  go_daddy: 'GoDaddy',
  aliyun: '阿里云',
  tencent_cloud: '腾讯云'
};

function newId(prefix: string) {
  return `${prefix}-${crypto.randomUUID()}`;
}

export function AcmeAccountsPanel({ config, onSaved, token }: AccountPanelProps) {
  const [editor, setEditor] = useState<AcmeAccountInput | null>(null);
  const [editing, setEditing] = useState(false);
  const [originalProvider, setOriginalProvider] = useState<AcmeAccountInput['provider'] | null>(null);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState<{ text: string; tone: 'success' | 'error' } | null>(null);
  const [error, setError] = useState('');

  function open(item?: AcmeAccountView) {
    setEditing(Boolean(item));
    setOriginalProvider(item?.provider ?? null);
    setEditor(item ? {
      id: item.id,
      name: item.name,
      provider: item.provider,
      environment: item.environment,
      email: item.email,
      key_algorithm: item.key_algorithm,
      eab_key_id: item.eab_key_id,
      eab_hmac_key: null
    } : {
      id: newId('acme'),
      name: '',
      provider: 'lets_encrypt',
      environment: 'staging',
      email: '',
      key_algorithm: 'ec256',
      eab_key_id: null,
      eab_hmac_key: null
    });
    setError('');
  }

  async function save() {
    if (!editor || !editor.name.trim() || !editor.email.trim()) {
      setError('名称和联系邮箱不能为空');
      return;
    }
    const canPreserveHmac = editing && originalProvider === 'google_cloud';
    if (editor.provider === 'google_cloud' && (!editor.eab_key_id?.trim() || (!canPreserveHmac && !editor.eab_hmac_key?.trim()))) {
      setError('Google Cloud 账户必须填写 EAB Key ID 和 HMAC Key');
      return;
    }
    const payload: AcmeAccountInput = editor.provider === 'lets_encrypt'
      ? { ...editor, eab_key_id: null, eab_hmac_key: null }
      : editor;
    setSaving(true);
    setError('');
    try {
      const saved = editing
        ? await updateAcmeAccount(token, payload.id, payload)
        : await createAcmeAccount(token, payload);
      onSaved(saved);
      setEditor(null);
      setNotice({ text: editing ? 'ACME 账户已保存' : 'ACME 账户已创建', tone: 'success' });
    } catch (cause) {
      setError(errorMessage(cause, '保存 ACME 账户失败'));
    } finally {
      setSaving(false);
    }
  }

  async function remove(id: string) {
    setNotice(null);
    try {
      onSaved(await deleteAcmeAccount(token, id));
      setNotice({ text: 'ACME 账户已删除', tone: 'success' });
    } catch (cause) {
      setNotice({ text: errorMessage(cause, '删除 ACME 账户失败'), tone: 'error' });
    }
  }

  return (
    <>
      {notice && <Notice tone={notice.tone}>{notice.text}</Notice>}
      <Panel>
        <PanelHeader action={<Button onClick={() => open()} variant="secondary"><Plus className="h-4 w-4" />新增账户</Button>} title="ACME 账户" />
        {config.acme_accounts.length ? (
          <Table className="min-w-[780px]">
            <TableHeader><TableRow><TableHead>名称</TableHead><TableHead>服务商</TableHead><TableHead>环境</TableHead><TableHead>联系邮箱</TableHead><TableHead>密钥算法</TableHead><TableHead className="text-right">操作</TableHead></TableRow></TableHeader>
            <tbody>{config.acme_accounts.map((item) => (
              <TableRow key={item.id}>
                <TableCell className="font-medium text-[color:var(--fg-base)]">{item.name}</TableCell>
                <TableCell>{acmeProviderLabels[item.provider]}</TableCell>
                <TableCell>{item.environment === 'production' ? '生产' : '测试'}</TableCell>
                <TableCell>{item.email}</TableCell>
                <TableCell>{item.key_algorithm === 'ec256' ? 'EC P-256' : 'RSA 2048'}</TableCell>
                <TableCell><div className="flex justify-end gap-1"><Button aria-label={`编辑 ${item.name}`} onClick={() => open(item)} size="icon" variant="ghost"><Pencil className="h-4 w-4" /></Button><ConfirmAction confirmLabel="删除" description="被托管证书引用时无法删除。" onConfirm={() => remove(item.id)} title={`删除 ${item.name}？`}><Button aria-label={`删除 ${item.name}`} size="icon" variant="ghost"><Trash2 className="h-4 w-4 text-[color:var(--tag-red-text)]" /></Button></ConfirmAction></div></TableCell>
              </TableRow>
            ))}</tbody>
          </Table>
        ) : <EmptyState description="证书申请前需要先建立 ACME 账户。" icon={KeyRound} title="还没有 ACME 账户" />}
      </Panel>
      <Dialog open={editor !== null} onOpenChange={(value) => !value && !saving && setEditor(null)}>
        {editor && <DialogContent title={editing ? '编辑 ACME 账户' : '新增 ACME 账户'}>
          <DialogBody><div className="grid gap-4 sm:grid-cols-2">
            <Field label="名称"><Input value={editor.name} onChange={(event) => setEditor({ ...editor, name: event.target.value })} /></Field>
            <Field label="服务商"><Select value={editor.provider} onChange={(event) => setEditor({ ...editor, provider: event.target.value as AcmeAccountInput['provider'] })}><option value="lets_encrypt">Let's Encrypt</option><option value="google_cloud">Google Cloud Public CA</option></Select></Field>
            <Field label="联系邮箱"><Input type="email" value={editor.email} onChange={(event) => setEditor({ ...editor, email: event.target.value })} /></Field>
            <Field label="环境"><Select value={editor.environment} onChange={(event) => setEditor({ ...editor, environment: event.target.value as AcmeAccountInput['environment'] })}><option value="staging">测试</option><option value="production">生产</option></Select></Field>
            <Field label="密钥算法"><Select value={editor.key_algorithm} onChange={(event) => setEditor({ ...editor, key_algorithm: event.target.value as AcmeAccountInput['key_algorithm'] })}><option value="ec256">EC P-256</option><option value="rsa2048">RSA 2048</option></Select></Field>
            {editor.provider === 'google_cloud' && <><Field label="EAB Key ID"><Input value={editor.eab_key_id ?? ''} onChange={(event) => setEditor({ ...editor, eab_key_id: event.target.value || null })} /></Field><Field label="EAB HMAC Key"><Input placeholder={editing ? '留空保留现有密钥' : ''} type="password" value={editor.eab_hmac_key ?? ''} onChange={(event) => setEditor({ ...editor, eab_hmac_key: event.target.value || null })} /></Field></>}
          </div>{error && <p className="txt-compact-small mt-4 text-[color:var(--tag-red-text)]" role="alert">{error}</p>}</DialogBody>
          <DialogFooter><Button disabled={saving} onClick={() => setEditor(null)} variant="secondary">取消</Button><Button disabled={saving} onClick={() => void save()}>{saving ? '保存中' : '保存'}</Button></DialogFooter>
        </DialogContent>}
      </Dialog>
    </>
  );
}

export function DnsAccountsPanel({ config, onSaved, token }: AccountPanelProps) {
  const [editor, setEditor] = useState<DnsAccountInput | null>(null);
  const [editing, setEditing] = useState(false);
  const [originalProvider, setOriginalProvider] = useState<DnsProvider | null>(null);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState<{ text: string; tone: 'success' | 'error' } | null>(null);
  const [error, setError] = useState('');

  function open(item?: DnsAccountView) {
    setEditing(Boolean(item));
    setOriginalProvider(item?.provider ?? null);
    setEditor(item ? { id: item.id, name: item.name, provider: item.provider, api_token: null, access_key: null, secret_key: null } : { id: newId('dns'), name: '', provider: 'cloudflare', api_token: null, access_key: null, secret_key: null });
    setError('');
  }

  async function save() {
    if (!editor || !editor.name.trim()) {
      setError('名称不能为空');
      return;
    }
    const requiresToken = editor.provider === 'cloudflare';
    const canPreserveCredentials = editing && originalProvider === editor.provider;
    if (!canPreserveCredentials && (requiresToken ? !editor.api_token?.trim() : !editor.access_key?.trim() || !editor.secret_key?.trim())) {
      setError(requiresToken ? 'Cloudflare API Token 不能为空' : 'Access Key 和 Secret Key 不能为空');
      return;
    }
    const payload = requiresToken
      ? { ...editor, access_key: null, secret_key: null }
      : { ...editor, api_token: null };
    setSaving(true);
    setError('');
    try {
      const saved = editing
        ? await updateDnsAccount(token, payload.id, payload)
        : await createDnsAccount(token, payload);
      onSaved(saved);
      setEditor(null);
      setNotice({ text: editing ? 'DNS 账户已保存' : 'DNS 账户已创建', tone: 'success' });
    } catch (cause) {
      setError(errorMessage(cause, '保存 DNS 账户失败'));
    } finally {
      setSaving(false);
    }
  }

  async function act(action: () => Promise<ProxyConfig | void>, success: string) {
    setNotice(null);
    try {
      const saved = await action();
      if (saved) onSaved(saved);
      setNotice({ text: success, tone: 'success' });
    } catch (cause) {
      setNotice({ text: errorMessage(cause, 'DNS 账户操作失败'), tone: 'error' });
    }
  }

  return (
    <>
      {notice && <Notice tone={notice.tone}>{notice.text}</Notice>}
      <Panel>
        <PanelHeader action={<Button onClick={() => open()} variant="secondary"><Plus className="h-4 w-4" />新增账户</Button>} title="DNS 账户" />
        {config.dns_accounts.length ? (
          <Table className="min-w-[680px]">
            <TableHeader><TableRow><TableHead>名称</TableHead><TableHead>服务商</TableHead><TableHead>凭据</TableHead><TableHead className="text-right">操作</TableHead></TableRow></TableHeader>
            <tbody>{config.dns_accounts.map((item) => (
              <TableRow key={item.id}>
                <TableCell className="font-medium text-[color:var(--fg-base)]">{item.name}</TableCell><TableCell>{dnsProviderLabels[item.provider]}</TableCell><TableCell>{item.provider === 'cloudflare' ? (item.api_token_configured ? 'API Token 已配置' : '未配置') : (item.access_key_configured && item.secret_key_configured ? 'Access/Secret Key 已配置' : '未配置')}</TableCell>
                <TableCell><div className="flex justify-end gap-1"><Button aria-label={`测试 ${item.name}`} onClick={() => void act(() => testDnsAccount(token, item.id), 'DNS 凭据测试通过')} size="icon" title="测试凭据" variant="ghost"><TestTube2 className="h-4 w-4" /></Button><Button aria-label={`编辑 ${item.name}`} onClick={() => open(item)} size="icon" variant="ghost"><Pencil className="h-4 w-4" /></Button><ConfirmAction confirmLabel="删除" description="被托管证书引用时无法删除。" onConfirm={() => act(() => deleteDnsAccount(token, item.id), 'DNS 账户已删除')} title={`删除 ${item.name}？`}><Button aria-label={`删除 ${item.name}`} size="icon" variant="ghost"><Trash2 className="h-4 w-4 text-[color:var(--tag-red-text)]" /></Button></ConfirmAction></div></TableCell>
              </TableRow>
            ))}</tbody>
          </Table>
        ) : <EmptyState description="自动 DNS 验证需要一个服务商账户。" icon={ServerCog} title="还没有 DNS 账户" />}
      </Panel>
      <Dialog open={editor !== null} onOpenChange={(value) => !value && !saving && setEditor(null)}>
        {editor && <DialogContent title={editing ? '编辑 DNS 账户' : '新增 DNS 账户'}><DialogBody><div className="grid gap-4 sm:grid-cols-2">
          <Field label="名称"><Input value={editor.name} onChange={(event) => setEditor({ ...editor, name: event.target.value })} /></Field><Field label="服务商"><Select value={editor.provider} onChange={(event) => setEditor({ ...editor, provider: event.target.value as DnsProvider })}>{Object.entries(dnsProviderLabels).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</Select></Field>
          {editor.provider === 'cloudflare' ? <Field className="sm:col-span-2" label="API Token"><Input placeholder={editing ? '留空保留现有 Token' : ''} type="password" value={editor.api_token ?? ''} onChange={(event) => setEditor({ ...editor, api_token: event.target.value || null })} /></Field> : <><Field label="Access Key"><Input placeholder={editing ? '留空保留现有 Access Key' : ''} type="password" value={editor.access_key ?? ''} onChange={(event) => setEditor({ ...editor, access_key: event.target.value || null })} /></Field><Field label="Secret Key"><Input placeholder={editing ? '留空保留现有 Secret Key' : ''} type="password" value={editor.secret_key ?? ''} onChange={(event) => setEditor({ ...editor, secret_key: event.target.value || null })} /></Field></>}
        </div>{error && <p className="txt-compact-small mt-4 text-[color:var(--tag-red-text)]" role="alert">{error}</p>}</DialogBody><DialogFooter><Button disabled={saving} onClick={() => setEditor(null)} variant="secondary">取消</Button><Button disabled={saving} onClick={() => void save()}>{saving ? '保存中' : '保存'}</Button></DialogFooter></DialogContent>}
      </Dialog>
    </>
  );
}
