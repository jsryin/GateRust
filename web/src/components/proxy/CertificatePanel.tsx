import { Clipboard, Pencil, Play, Plus, RefreshCw, ShieldCheck, Trash2 } from 'lucide-react';
import { useState } from 'react';
import {
  continueCertificate,
  createCertificate,
  deleteCertificate,
  issueCertificate,
  updateCertificate
} from '../../lib/api';
import { errorMessage } from '../../lib/errors';
import type {
  CertificateConfig,
  CertificateRuntimeStatus,
  CertificateStatus,
  ProxyConfig
} from '../../lib/types';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { ConfirmAction } from '../ui/ConfirmAction';
import { Dialog, DialogBody, DialogContent, DialogFooter } from '../ui/Dialog';
import { CheckboxField, Field, Input, Select, Textarea } from '../ui/Fields';
import { Notice } from '../ui/Notice';
import { EmptyState, Panel, PanelHeader } from '../ui/Panel';
import { Table, TableCell, TableHead, TableHeader, TableRow } from '../ui/Table';

interface CertificatePanelProps {
  config: ProxyConfig;
  onRuntimeRefresh: () => void;
  onSaved: (config: ProxyConfig) => void;
  runtime: CertificateRuntimeStatus[];
  token: string;
}

const statusLabels: Record<CertificateStatus, string> = {
  idle: '未申请',
  issuing: '申请中',
  waiting_dns: '等待解析',
  valid: '有效',
  renewing: '续签中',
  failed: '失败',
  expired: '已过期'
};

function emptyCertificate(config: ProxyConfig): CertificateConfig {
  return {
    id: `certificate-${crypto.randomUUID()}`,
    name: '',
    domains: [],
    acme_account_id: config.acme_accounts[0]?.id ?? '',
    validation: null,
    auto_renew: false,
    migration_error: null
  };
}

function expiry(value: number | null | undefined) {
  return value ? new Intl.DateTimeFormat('zh-CN', { dateStyle: 'medium', timeStyle: 'short' }).format(value * 1000) : '-';
}

function tone(status: CertificateStatus) {
  if (status === 'valid') return 'green' as const;
  if (status === 'failed' || status === 'expired') return 'red' as const;
  if (status === 'issuing' || status === 'renewing' || status === 'waiting_dns') return 'orange' as const;
  return 'neutral' as const;
}

export function CertificatePanel({ config, onRuntimeRefresh, onSaved, runtime, token }: CertificatePanelProps) {
  const [editor, setEditor] = useState<CertificateConfig | null>(null);
  const [editing, setEditing] = useState(false);
  const [domains, setDomains] = useState('');
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState<{ text: string; tone: 'success' | 'error' } | null>(null);
  const [error, setError] = useState('');
  const [recordsFor, setRecordsFor] = useState<string | null>(null);

  const statuses = new Map(runtime.map((status) => [status.certificate_id, status]));
  const recordStatus = recordsFor ? statuses.get(recordsFor) : undefined;

  function open(item?: CertificateConfig) {
    const next = item ? structuredClone(item) : emptyCertificate(config);
    setEditing(Boolean(item));
    setEditor(next);
    setDomains(next.domains.join('\n'));
    setError('');
  }

  async function save() {
    if (!editor) return;
    const parsedDomains = domains.split(/[\s,]+/).filter(Boolean);
    if (!editor.name.trim() || !parsedDomains.length || !editor.acme_account_id || !editor.validation) {
      setError('名称、域名、ACME 账户和验证方式不能为空');
      return;
    }
    const payload: CertificateConfig = {
      ...editor,
      domains: parsedDomains,
      auto_renew: editor.validation.method === 'manual' ? false : editor.auto_renew,
      migration_error: null
    };
    setSaving(true);
    setError('');
    try {
      const saved = editing
        ? await updateCertificate(token, payload.id, payload)
        : await createCertificate(token, payload);
      onSaved(saved);
      setEditor(null);
      setNotice({ text: editing ? '托管证书已保存' : '托管证书已创建，请点击申请', tone: 'success' });
      onRuntimeRefresh();
    } catch (cause) {
      setError(errorMessage(cause, '保存托管证书失败'));
    } finally {
      setSaving(false);
    }
  }

  async function action(operation: () => Promise<ProxyConfig | void>, success: string) {
    setNotice(null);
    try {
      const saved = await operation();
      if (saved) onSaved(saved);
      setNotice({ text: success, tone: 'success' });
      onRuntimeRefresh();
    } catch (cause) {
      setNotice({ text: errorMessage(cause, '证书操作失败'), tone: 'error' });
    }
  }

  const hasAccounts = config.acme_accounts.length > 0;
  return (
    <>
      {notice && <Notice tone={notice.tone}>{notice.text}</Notice>}
      <Panel>
        <PanelHeader action={<Button disabled={!hasAccounts} onClick={() => open()} variant="secondary"><Plus className="h-4 w-4" />新增证书</Button>} title="托管证书" />
        {config.certificates.length ? (
          <Table className="min-w-[1120px]">
            <TableHeader><TableRow><TableHead>名称</TableHead><TableHead>域名</TableHead><TableHead>ACME 账户</TableHead><TableHead>验证方式</TableHead><TableHead>自动续签</TableHead><TableHead>状态</TableHead><TableHead>过期时间</TableHead><TableHead className="text-right">操作</TableHead></TableRow></TableHeader>
            <tbody>{config.certificates.map((item) => {
              const state = statuses.get(item.id);
              const status = state?.status ?? 'idle';
              const active = status === 'issuing' || status === 'renewing';
              const account = config.acme_accounts.find((entry) => entry.id === item.acme_account_id);
              const dnsAccountId = item.validation?.method === 'dns_account' ? item.validation.dns_account_id : null;
              const dnsName = dnsAccountId
                ? config.dns_accounts.find((entry) => entry.id === dnsAccountId)?.name ?? 'DNS 账户不存在'
                : item.validation?.method === 'manual' ? '手动解析' : '待迁移';
              return <TableRow key={item.id}>
                <TableCell className="font-medium text-[color:var(--fg-base)]">{item.name}</TableCell><TableCell className="max-w-64 truncate" title={item.domains.join(', ')}>{item.domains.join(', ')}</TableCell><TableCell>{account?.name ?? '账户不存在'}</TableCell><TableCell>{dnsName}</TableCell><TableCell>{item.auto_renew ? '启用' : '关闭'}</TableCell><TableCell title={state?.last_error ?? undefined}><Badge tone={tone(status)}>{statusLabels[status]}</Badge></TableCell><TableCell>{expiry(state?.expires_at)}</TableCell>
                <TableCell><div className="flex justify-end gap-1">
                  {status === 'waiting_dns' && <Button aria-label={`查看 ${item.name} DNS 记录`} onClick={() => setRecordsFor(item.id)} size="icon" title="查看 DNS 记录" variant="ghost"><Clipboard className="h-4 w-4" /></Button>}
                  {status === 'waiting_dns' && <Button aria-label={`继续验证 ${item.name}`} onClick={() => void action(() => continueCertificate(token, item.id), '已提交继续验证')} size="icon" title="继续验证" variant="ghost"><RefreshCw className="h-4 w-4" /></Button>}
                  <Button aria-label={`申请 ${item.name}`} disabled={active || !item.validation} onClick={() => void action(() => issueCertificate(token, item.id), '证书申请已开始')} size="icon" title={state?.expires_at ? '重新申请' : '申请证书'} variant="ghost"><Play className="h-4 w-4" /></Button>
                  <Button aria-label={`编辑 ${item.name}`} disabled={active} onClick={() => open(item)} size="icon" variant="ghost"><Pencil className="h-4 w-4" /></Button>
                  <ConfirmAction confirmLabel="删除" description="被域名路由引用时无法删除。" onConfirm={() => action(() => deleteCertificate(token, item.id), '托管证书已删除')} title={`删除 ${item.name}？`}><Button aria-label={`删除 ${item.name}`} disabled={active} size="icon" variant="ghost"><Trash2 className="h-4 w-4 text-[color:var(--tag-red-text)]" /></Button></ConfirmAction>
                </div></TableCell>
              </TableRow>;
            })}</tbody>
          </Table>
        ) : <EmptyState description={hasAccounts ? '创建后点击申请才会向 CA 发起订单。' : '请先创建 ACME 账户。'} icon={ShieldCheck} title="还没有托管证书" />}
      </Panel>

      <Dialog open={editor !== null} onOpenChange={(value) => !value && !saving && setEditor(null)}>
        {editor && <DialogContent title={editing ? '编辑托管证书' : '新增托管证书'}><DialogBody><div className="grid gap-4 sm:grid-cols-2">
          <Field label="名称"><Input value={editor.name} onChange={(event) => setEditor({ ...editor, name: event.target.value })} /></Field>
          <Field label="ACME 账户"><Select value={editor.acme_account_id} onChange={(event) => setEditor({ ...editor, acme_account_id: event.target.value })}><option value="">请选择</option>{config.acme_accounts.map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}</Select></Field>
          <Field className="sm:col-span-2" label="域名（每行一个，泛域名使用 *.example.com）"><Textarea rows={4} value={domains} onChange={(event) => setDomains(event.target.value)} /></Field>
          <Field label="验证方式"><Select value={editor.validation?.method ?? ''} onChange={(event) => {
            const method = event.target.value;
            setEditor({ ...editor, validation: method === 'manual' ? { method: 'manual' } : method === 'dns_account' ? { method: 'dns_account', dns_account_id: config.dns_accounts[0]?.id ?? '' } : null, auto_renew: method === 'manual' ? false : editor.auto_renew });
          }}><option value="">请选择</option><option value="dns_account" disabled={!config.dns_accounts.length}>DNS 账户</option><option value="manual">手动解析</option></Select></Field>
          {editor.validation?.method === 'dns_account' && <Field label="DNS 账户"><Select value={editor.validation.dns_account_id} onChange={(event) => setEditor({ ...editor, validation: { method: 'dns_account', dns_account_id: event.target.value } })}><option value="">请选择</option>{config.dns_accounts.map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}</Select></Field>}
          <CheckboxField checked={editor.auto_renew} disabled={editor.validation?.method !== 'dns_account'} label="自动续签（到期前 14 天开始）" onChange={(event) => setEditor({ ...editor, auto_renew: event.target.checked })} />
        </div>{error && <p className="txt-compact-small mt-4 text-[color:var(--tag-red-text)]" role="alert">{error}</p>}</DialogBody><DialogFooter><Button disabled={saving} onClick={() => setEditor(null)} variant="secondary">取消</Button><Button disabled={saving} onClick={() => void save()}>{saving ? '保存中' : '保存'}</Button></DialogFooter></DialogContent>}
      </Dialog>

      <Dialog open={recordsFor !== null} onOpenChange={(value) => !value && setRecordsFor(null)}>
        {recordsFor && <DialogContent title="DNS TXT 解析值"><DialogBody><div className="space-y-3">{recordStatus?.manual_records.map((record) => <div className="rounded-md border border-[color:var(--border-base)] bg-[var(--bg-subtle)] p-3" key={`${record.name}-${record.value}`}><div className="txt-compact-xsmall text-[color:var(--fg-muted)]">记录名</div><div className="txt-compact-small mt-1 flex items-start gap-2"><code className="min-w-0 flex-1 break-all">{record.name}</code><Button aria-label="复制记录名" onClick={() => void navigator.clipboard.writeText(record.name)} size="icon" variant="ghost"><Clipboard className="h-4 w-4" /></Button></div><div className="txt-compact-xsmall mt-3 text-[color:var(--fg-muted)]">记录值</div><div className="txt-compact-small mt-1 flex items-start gap-2"><code className="min-w-0 flex-1 break-all">{record.value}</code><Button aria-label="复制记录值" onClick={() => void navigator.clipboard.writeText(record.value)} size="icon" variant="ghost"><Clipboard className="h-4 w-4" /></Button></div></div>)}</div>{recordStatus?.last_error && <p className="txt-compact-small mt-4 text-[color:var(--tag-red-text)]">{recordStatus.last_error}</p>}</DialogBody><DialogFooter><Button onClick={() => setRecordsFor(null)} variant="secondary">关闭</Button><Button onClick={() => void action(() => continueCertificate(token, recordsFor), '已提交继续验证')}>继续验证</Button></DialogFooter></DialogContent>}
      </Dialog>
    </>
  );
}
