import { X } from 'lucide-react';
import type { Requirement, RequirementPriority } from '@shared/types';
import { StatusBadge, PriorityBadge, TypeBadge } from '../ui/Badge';
import { EditableText, EditableSelect } from '../ui/EditableField';
import { STATUS_ORDER, STATUS_CONFIG, PRIORITY_CONFIG } from '../../lib/constants';
import { useUpdateRequirement } from '../../hooks/useRequirements';
import { cn } from '../../lib/utils';

const PRIORITIES: RequirementPriority[] = ['High', 'Medium', 'Low'];

interface DetailHeaderProps {
  requirement: Requirement;
  onClose: () => void;
}

export function DetailHeader({ requirement, onClose }: DetailHeaderProps) {
  const updateReq = useUpdateRequirement();
  const reqId = requirement.spec_id ?? requirement.id;

  function save(data: Partial<Requirement>) {
    updateReq.mutate({ id: reqId, data });
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

      {/* Editable title */}
      <div className="mb-3">
        <EditableText
          value={requirement.title}
          onSave={(title) => save({ title })}
          className="text-lg font-semibold text-content leading-snug"
          inputClassName="text-lg font-semibold"
          placeholder="Untitled requirement"
        />
      </div>

      {/* Badges row — clickable to change */}
      <div className="flex items-center gap-3 flex-wrap">
        <EditableSelect
          value={requirement.status}
          options={STATUS_ORDER}
          onSave={(status) => save({ status })}
          renderValue={(s) => <StatusBadge status={s} />}
          renderOption={(s) => (
            <span className="flex items-center gap-2">
              <span className={cn('h-1.5 w-1.5 rounded-full', STATUS_CONFIG[s].dot)} />
              {STATUS_CONFIG[s].label}
            </span>
          )}
        />
        <EditableSelect
          value={requirement.priority}
          options={PRIORITIES}
          onSave={(priority) => save({ priority })}
          renderValue={(p) => <PriorityBadge priority={p} />}
          renderOption={(p) => (
            <span className={cn('flex items-center gap-2', PRIORITY_CONFIG[p].color)}>
              {PRIORITY_CONFIG[p].label}
            </span>
          )}
        />
        <TypeBadge type={requirement.req_type} />
      </div>
    </div>
  );
}
