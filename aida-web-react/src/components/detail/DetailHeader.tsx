import { X, ExternalLink, ListPlus, Check, Sparkles, Loader2 } from 'lucide-react';
import { useState } from 'react';
import type { Requirement, RequirementPriority, RequirementType } from '@shared/types';
import { StatusBadge, PriorityBadge, TypeBadge } from '../ui/Badge';
import { EditableText, EditableSelect } from '../ui/EditableField';
import { STATUS_ORDER, STATUS_CONFIG, PRIORITY_CONFIG, TYPE_CONFIG } from '../../lib/constants';
import { useUpdateRequirement } from '../../hooks/useRequirements';
import { useAddToQueue } from '../../hooks/useQueue';
import { useEvaluateRequirement } from '../../hooks/useEvaluation';
import { cn } from '../../lib/utils';

const PRIORITIES: RequirementPriority[] = ['High', 'Medium', 'Low'];
const TYPES = Object.keys(TYPE_CONFIG) as RequirementType[];

interface DetailHeaderProps {
  requirement: Requirement;
  onClose: () => void;
  hideClose?: boolean;
}

export function DetailHeader({ requirement, onClose, hideClose }: DetailHeaderProps) {
  const updateReq = useUpdateRequirement();
  const addToQueue = useAddToQueue();
  const evaluate = useEvaluateRequirement();
  const [queued, setQueued] = useState(false);
  const reqId = requirement.spec_id ?? requirement.id;

  function save(data: Partial<Requirement>) {
    updateReq.mutate({ id: reqId, data });
  }

  return (
    <div className="border-b border-edge px-6 py-4 shrink-0">
      {/* Top row: spec_id + actions */}
      <div className="flex items-center justify-between mb-3">
        <span className="text-xs font-mono text-content-muted">{requirement.spec_id}</span>
        <div className="flex items-center gap-1">
          <button
            onClick={() => evaluate.mutate(reqId)}
            disabled={evaluate.isPending}
            title="AI Evaluate"
            className="flex h-7 w-7 items-center justify-center rounded-lg text-content-muted hover:text-amber-400 hover:bg-amber-400/10 disabled:opacity-50 transition-colors cursor-pointer disabled:cursor-not-allowed"
          >
            {evaluate.isPending ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : evaluate.isSuccess ? (
              <Check className="h-3.5 w-3.5 text-green-500" />
            ) : (
              <Sparkles className="h-3.5 w-3.5" />
            )}
          </button>
          <button
            onClick={() => {
              addToQueue.mutate({ requirement_id: requirement.id });
              setQueued(true);
              setTimeout(() => setQueued(false), 2000);
            }}
            title="Add to queue"
            className="flex h-7 w-7 items-center justify-center rounded-lg text-content-muted hover:text-accent hover:bg-accent/10 transition-colors cursor-pointer"
          >
            {queued ? (
              <Check className="h-3.5 w-3.5 text-green-500" />
            ) : (
              <ListPlus className="h-3.5 w-3.5" />
            )}
          </button>
          {!hideClose && (
            <button
              onClick={() => window.open(`/req/${reqId}`, '_blank')}
              title="Open in new tab"
              className="flex h-7 w-7 items-center justify-center rounded-lg text-content-muted hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
            >
              <ExternalLink className="h-3.5 w-3.5" />
            </button>
          )}
          {!hideClose && (
            <button
              onClick={onClose}
              className="flex h-7 w-7 items-center justify-center rounded-lg text-content-muted hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
            >
              <X className="h-4 w-4" />
            </button>
          )}
        </div>
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
        <EditableSelect
          value={requirement.req_type}
          options={TYPES}
          onSave={(req_type) => save({ req_type })}
          renderValue={(t) => <TypeBadge type={t} />}
          renderOption={(t) => (
            <span className={cn('flex items-center gap-2', TYPE_CONFIG[t].color)}>
              {TYPE_CONFIG[t].label}
            </span>
          )}
        />
      </div>
    </div>
  );
}
