import { Globe2, Pencil, Plus, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { createRoute, deleteRoute, updateRoute } from '../../lib/api';
import { errorMessage } from '../../lib/errors';
import type { ProxyConfig, RouteConfig } from '../../lib/types';
import { Button } from '../ui/Button';
import { ConfirmAction } from '../ui/ConfirmAction';
import { Dialog, DialogBody, DialogContent, DialogFooter } from '../ui/Dialog';
import { Field, Input, Select } from '../ui/Fields';
import { Notice } from '../ui/Notice';
import { EmptyState, Panel, PanelHeader } from '../ui/Panel';
import { Table, TableCell, TableHead, TableHeader, TableRow } from '../ui/Table';

interface RoutePanelProps {
  config: ProxyConfig;
  onSaved: (config: ProxyConfig) => void;
  token: string;
}

const emptyRoute = (): RouteConfig => ({
  name: '',
  host: '',
  path_prefix: '/',
  upstream: 'http://127.0.0.1:3000',
  certificate_id: null
});

export function RoutePanel({ config, onSaved, token }: RoutePanelProps) {
  const [editor, setEditor] = useState<RouteConfig | null>(null);
  const [originalName, setOriginalName] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState<{ text: string; tone: 'success' | 'error' } | null>(null);
  const [error, setError] = useState('');

  function open(item?: RouteConfig) {
    setOriginalName(item?.name ?? null);
    setEditor(item ? { ...item } : emptyRoute());
    setError('');
  }

  async function save() {
    if (!editor || !editor.name.trim() || !editor.host.trim() || !editor.upstream.trim()) {
      setError('名称、域名和上游地址不能为空');
      return;
    }
    setSaving(true);
    setError('');
    try {
      const saved = originalName
        ? await updateRoute(token, originalName, editor)
        : await createRoute(token, editor);
      onSaved(saved);
      setEditor(null);
      setNotice({ text: originalName ? '域名路由已保存' : '域名路由已创建', tone: 'success' });
    } catch (cause) {
      setError(errorMessage(cause, '保存域名路由失败'));
    } finally {
      setSaving(false);
    }
  }

  async function remove(name: string) {
    setNotice(null);
    try {
      onSaved(await deleteRoute(token, name));
      setNotice({ text: '域名路由已删除', tone: 'success' });
    } catch (cause) {
      setNotice({ text: errorMessage(cause, '删除域名路由失败'), tone: 'error' });
    }
  }

  return (
    <>
      {notice && <Notice tone={notice.tone}>{notice.text}</Notice>}
      <Panel>
        <PanelHeader action={<Button onClick={() => open()} variant="secondary"><Plus className="h-4 w-4" />新增路由</Button>} title="域名路由" />
        {config.routes.length ? <Table className="min-w-[760px]">
          <TableHeader><TableRow><TableHead>名称</TableHead><TableHead>Host / Path</TableHead><TableHead>上游</TableHead><TableHead>SSL 证书</TableHead><TableHead className="text-right">操作</TableHead></TableRow></TableHeader>
          <tbody>{config.routes.map((item) => <TableRow key={item.name}><TableCell className="font-medium text-[color:var(--fg-base)]">{item.name}</TableCell><TableCell><code className="text-xs">{item.host}{item.path_prefix}</code></TableCell><TableCell>{item.upstream}</TableCell><TableCell>{config.certificates.find((certificate) => certificate.id === item.certificate_id)?.name ?? '不启用'}</TableCell><TableCell><div className="flex justify-end gap-1"><Button aria-label={`编辑 ${item.name}`} onClick={() => open(item)} size="icon" variant="ghost"><Pencil className="h-4 w-4" /></Button><ConfirmAction confirmLabel="删除" description="删除后该域名和路径将立即停止代理。" onConfirm={() => remove(item.name)} title={`删除 ${item.name}？`}><Button aria-label={`删除 ${item.name}`} size="icon" variant="ghost"><Trash2 className="h-4 w-4 text-[color:var(--tag-red-text)]" /></Button></ConfirmAction></div></TableCell></TableRow>)}</tbody>
        </Table> : <EmptyState description="路由可指向本地隧道端口或公网 HTTP(S) 上游。" icon={Globe2} title="还没有域名路由" />}
      </Panel>
      <Dialog open={editor !== null} onOpenChange={(value) => !value && !saving && setEditor(null)}>
        {editor && <DialogContent title={originalName ? '编辑域名路由' : '新增域名路由'}><DialogBody><div className="grid gap-4 sm:grid-cols-2"><Field label="名称"><Input value={editor.name} onChange={(event) => setEditor({ ...editor, name: event.target.value })} /></Field><Field label="域名"><Input placeholder="example.com" value={editor.host} onChange={(event) => setEditor({ ...editor, host: event.target.value })} /></Field><Field label="路径前缀"><Input value={editor.path_prefix} onChange={(event) => setEditor({ ...editor, path_prefix: event.target.value })} /></Field><Field label="上游地址"><Input value={editor.upstream} onChange={(event) => setEditor({ ...editor, upstream: event.target.value })} /></Field><Field label="SSL 证书"><Select value={editor.certificate_id ?? ''} onChange={(event) => setEditor({ ...editor, certificate_id: event.target.value || null })}><option value="">不启用</option>{config.certificates.map((certificate) => <option key={certificate.id} value={certificate.id}>{certificate.name}</option>)}</Select></Field></div>{error && <p className="txt-compact-small mt-4 text-[color:var(--tag-red-text)]" role="alert">{error}</p>}</DialogBody><DialogFooter><Button disabled={saving} onClick={() => setEditor(null)} variant="secondary">取消</Button><Button disabled={saving} onClick={() => void save()}>{saving ? '保存中' : '保存'}</Button></DialogFooter></DialogContent>}
      </Dialog>
    </>
  );
}
