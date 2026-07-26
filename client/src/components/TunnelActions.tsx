import { LoaderCircle } from 'lucide-react';
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
  const disableMode = enabledCount > 0 || action === 'disable';
  const handleAction = disableMode ? onDisable : onEnable;

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
      <button
        className={disableMode ? 'danger-button' : 'primary-button'}
        disabled={action !== null || (!disableMode && selectedIdleCount === 0)}
        onClick={() => void handleAction()}
        type="button"
      >
        {action && <LoaderCircle className="spin" size={16} />}
        {disableMode ? '停用' : '启用'}
      </button>
    </div>
  );
}
