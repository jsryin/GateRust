import { LoaderCircle, Play, PowerOff } from 'lucide-react';
import { useEffect, useRef } from 'react';

type TunnelAction = 'enable' | 'disable' | null;

interface TunnelActionsProps {
  action: TunnelAction;
  enabledCount: number;
  idleCount: number;
  onEnable: () => Promise<void>;
  onDisable: () => Promise<void>;
  onToggleAll: () => void;
  selectedIdleCount: number;
}

export function TunnelActions({
  action,
  enabledCount,
  idleCount,
  onEnable,
  onDisable,
  onToggleAll,
  selectedIdleCount
}: TunnelActionsProps) {
  const selectAllRef = useRef<HTMLInputElement>(null);
  const allIdleSelected = idleCount > 0 && selectedIdleCount === idleCount;
  const selectionDisabled = idleCount === 0 || action !== null;

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
          {selectedIdleCount ? `已选 ${selectedIdleCount} 条` : `${enabledCount} 条已启用`}
        </span>
      </div>
      <div className="action-buttons">
        <button
          className="primary-button"
          disabled={action !== null || selectedIdleCount === 0}
          onClick={() => void onEnable()}
          type="button"
        >
          {action === 'enable' ? <LoaderCircle className="spin" size={16} /> : <Play size={15} />}
          启用所选
        </button>
        {enabledCount > 0 && (
          <button
            className="danger-button"
            disabled={action !== null}
            onClick={() => void onDisable()}
            type="button"
          >
            {action === 'disable' ? <LoaderCircle className="spin" size={16} /> : <PowerOff size={15} />}
            停用全部
          </button>
        )}
      </div>
    </div>
  );
}
