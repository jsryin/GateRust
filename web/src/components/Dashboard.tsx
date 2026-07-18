import { Cable, CircleCheck, CircleOff, KeyRound, Route, ShieldCheck } from 'lucide-react';
import type { ConfigSnapshot, Dashboard as DashboardState } from '../lib/types';
import { Badge } from './ui/Badge';
import { Panel, PanelHeader } from './ui/Panel';

interface DashboardProps {
  config: ConfigSnapshot;
  dashboard: DashboardState | null;
}

export function Dashboard({ config, dashboard }: DashboardProps) {
  const metrics = [
    { label: '分组', value: dashboard?.groups ?? config.tunnel?.groups.length ?? 0, icon: KeyRound },
    { label: '隧道', value: dashboard?.tunnels ?? config.tunnel?.tunnels.length ?? 0, icon: Cable },
    { label: '托管证书', value: dashboard?.certificates ?? config.proxy?.certificates.length ?? 0, icon: ShieldCheck },
    { label: '代理路由', value: dashboard?.routes ?? config.proxy?.routes.length ?? 0, icon: Route }
  ];

  return (
    <div className="space-y-4">
      <div className="flex justify-end">
        <div className="txt-compact-xsmall-plus flex items-center gap-2 text-[color:var(--fg-subtle)]">
          <span className="relative flex h-2 w-2">
            {dashboard && <span className="absolute h-full w-full animate-ping rounded-full bg-emerald-400 opacity-60" />}
            <span className={`relative h-2 w-2 rounded-full ${dashboard ? 'bg-emerald-500' : 'bg-zinc-400'}`} />
          </span>
          {dashboard ? '实时状态' : '等待实时状态'}
        </div>
      </div>

      <section className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        {metrics.map(({ icon: Icon, label, value }) => (
          <article className="rounded-lg bg-[var(--bg-base)] p-4 shadow-[var(--elevation-card-rest)]" key={label}>
            <div className="flex items-start justify-between gap-3">
              <div>
                <div className="txt-compact-small-plus text-[color:var(--fg-subtle)]">{label}</div>
                <div className="mt-3 text-2xl font-medium leading-8">{value}</div>
              </div>
              <div className="grid h-8 w-8 place-items-center rounded-md bg-[var(--bg-component)] text-[color:var(--fg-muted)] shadow-[var(--borders-base)]">
                <Icon className="h-4 w-4" />
              </div>
            </div>
          </article>
        ))}
      </section>

      <Panel>
        <PanelHeader description="当前进程的启用状态与配置载入情况" title="模块状态" />
        <div className="px-5 sm:px-6">
          <ModuleStatus
            description={config.tunnel ? `${config.tunnel.quic.bind} · 已载入配置` : '尚未创建配置'}
            enabled={Boolean(dashboard?.tunnel_enabled)}
            title="QUIC 内网穿透"
          />
          <ModuleStatus
            description={config.proxy ? `${config.proxy.proxy.http_bind} / ${config.proxy.proxy.https_bind}` : '尚未创建配置'}
            enabled={Boolean(dashboard?.proxy_enabled)}
            title="反向代理与自动 SSL"
          />
        </div>
      </Panel>
    </div>
  );
}

function ModuleStatus({ description, enabled, title }: { description: string; enabled: boolean; title: string }) {
  const Icon = enabled ? CircleCheck : CircleOff;

  return (
    <div className="grid min-h-20 grid-cols-[32px_minmax(0,1fr)_auto] items-center gap-3 border-b border-[color:var(--border-base)] last:border-b-0">
      <Icon className={`h-5 w-5 ${enabled ? 'text-emerald-500' : 'text-[color:var(--fg-muted)]'}`} />
      <div className="min-w-0">
        <strong className="txt-compact-small-plus font-medium">{title}</strong>
        <p className="txt-compact-xsmall mt-0.5 truncate text-[color:var(--fg-muted)]">{description}</p>
      </div>
      <Badge tone={enabled ? 'green' : 'neutral'}>{enabled ? '运行中' : '未启用'}</Badge>
    </div>
  );
}
