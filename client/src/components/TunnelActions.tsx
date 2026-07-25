import { LoaderCircle } from 'lucide-react';
import { useEffect, useRef } from 'react';

type TunnelAction = 'connect' | 'disconnect' | null;

interface TunnelActionsProps {
  action: TunnelAction;
  connectedCount: number;
  idleCount: number;
  onConnect: () => Promise<void>;
  onDisconnect: () => Promise<void>;
  onToggleAll: () => void;
  selectedIdleCount: number;
}

export function TunnelActions({
  action,
  connectedCount,
  idleCount,
  onConnect,
  onDisconnect,
  onToggleAll,
  selectedIdleCount
}: TunnelActionsProps) {
  const selectAllRef = useRef<HTMLInputElement>(null);
  const allIdleSelected = idleCount > 0 && selectedIdleCount === idleCount;
  const selectionDisabled = idleCount === 0 || action !== null;
  const disconnectMode = connectedCount > 0 || action === 'disconnect';
  const handleAction = disconnectMode ? onDisconnect : onConnect;

  useEffect(() => {
    if (selectAllRef.current) {
      selectAllRef.current.indeterminate = selectedIdleCount > 0 && !allIdleSelected;
    }
  }, [allIdleSelected, selectedIdleCount]);

  return (
    <div className="action-bar">
      <div className="action-selection">
        <label className={`select-all ${selectionDisabled ? 'disabled' : ''}`}>
          <input
            aria-label="全选空闲隧道"
            checked={allIdleSelected}
            disabled={selectionDisabled}
            onChange={onToggleAll}
            ref={selectAllRef}
            type="checkbox"
          />
          <span>全选</span>
        </label>
        <span className="action-status">
          {selectedIdleCount ? `已选 ${selectedIdleCount} 条` : `${connectedCount} 条已连接`}
        </span>
      </div>
      <button
        className={disconnectMode ? 'danger-button' : 'primary-button'}
        disabled={action !== null || (!disconnectMode && selectedIdleCount === 0)}
        onClick={() => void handleAction()}
        type="button"
      >
        {action && <LoaderCircle className="spin" size={16} />}
        {disconnectMode ? '断开' : '连接'}
      </button>
    </div>
  );
}
