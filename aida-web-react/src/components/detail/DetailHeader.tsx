import { X } from 'lucide-react';
import type { Requirement, RequirementStatus } from '@shared/types';
import { StatusBadge, PriorityBadge, TypeBadge } from '../ui/Badge';
import { STATUS_ORDER } from '../../lib/constants';
import { useUpdateRequirement } from '../../hooks/useRequirements';

interface DetailHeaderProps {
  requirement: Requirement;
  onClose: () => void;
}

export function DetailHeader({ requirement, onClose }: DetailHeaderProps) {
  const updateReq = useUpdateRequirement();

  function handleStatusChange(newStatus: RequirementStatus) {
    updateReq.mutate({
      id: requirement.spec_id ?? requirement.id,
      data: { status: newStatus },
    });
  }

  return (
    <div className="border-b border-edge px-6 py-4 shrink-0">
      {/* Top row: spec_id + close */}
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs font-mono text-content-muted">{requirement.spec_id}</span>
        <button
          onClick={onClose}
          className="flex h-7 w-7 items-center justify-center rounded-lg text-content-muted hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      {/* Title */}
      <h2 className="text-lg font-semibold text-content mb-3 leading-snug">
        {requirement.title}
      </h2>

      {/* Badges row */}
      <div className="flex items-center gap-3 flex-wrap">
        {/* Status dropdown */}
        <div className="relative group">
          <StatusBadge status={requirement.status} />
          <div className="absolute top-full left-0 mt-1 hidden group-hover:block z-10">
            <div className="rounded-lg border border-edge bg-surface-alt shadow-xl shadow-black/20 py-1 min-w-[140px]">
              {STATUS_ORDER.map((status) => (
                <button
                  key={status}
                  onClick={() => handleStatusChange(status)}
                  className="w-full px-3 py-1.5 text-left text-xs text-content-secondary hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
                >
                  {status}
                </button>
              ))}
            </div>
          </div>
        </div>
        <PriorityBadge priority={requirement.priority} />
        <TypeBadge type={requirement.req_type} />
      </div>
    </div>
  );
}
