import type { ClientStatus, ClientStatusState } from '../../../shared/types';

interface StatusPaneProps {
  status: ClientStatus;
}

const labels: Record<ClientStatusState, string> = {
  starting: '正在启动',
  unconfigured: '等待配置',
  connecting: '正在连接',
  connected: '已连接',
  reconnecting: '等待重连',
  stopped: '已停止',
  offline: '后台已断开'
};

export function StatusPane({ status }: StatusPaneProps) {
  const connectionDetail = [status.server, status.device_id].filter(Boolean).join(' / ');
  const retryDetail = status.retry_seconds ? `${status.retry_seconds} 秒后重试` : '';
  const detail = status.message
    ? [status.message, retryDetail].filter(Boolean).join(' / ')
    : connectionDetail || retryDetail;
  const tone =
    status.state === 'connected'
      ? 'connected'
      : status.state === 'stopped' || status.state === 'offline'
        ? 'error'
        : status.state === 'reconnecting'
          ? 'warning'
          : 'pending';

  return (
    <div aria-live="polite" className="status-pane" title={detail || undefined}>
      <i aria-hidden="true" className={tone} />
      <span>
        <strong>{labels[status.state]}</strong>
        {detail && <small>{detail}</small>}
      </span>
    </div>
  );
}
