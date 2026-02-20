import { cn } from '../../lib/utils';
import { STATUS_CONFIG, PRIORITY_CONFIG, TYPE_CONFIG } from '../../lib/constants';
import type { RequirementStatus, RequirementPriority, RequirementType } from '@shared/types';

export function StatusBadge({ status }: { status: RequirementStatus }) {
  const config = STATUS_CONFIG[status];
  return (
    <span className={cn('inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-xs font-medium', config.bg, config.color)}>
      <span className={cn('h-1.5 w-1.5 rounded-full', config.dot)} />
      {config.label}
    </span>
  );
}

export function PriorityBadge({ priority }: { priority: RequirementPriority }) {
  const config = PRIORITY_CONFIG[priority];
  return (
    <span className={cn('inline-flex items-center gap-1 text-xs font-medium', config.color)}>
      {config.label}
    </span>
  );
}

export function TypeBadge({ type }: { type: RequirementType }) {
  const config = TYPE_CONFIG[type];
  return (
    <span className={cn('inline-flex items-center rounded-md px-2 py-0.5 text-[11px] font-medium', config.bg, config.color)}>
      {config.label}
    </span>
  );
}
