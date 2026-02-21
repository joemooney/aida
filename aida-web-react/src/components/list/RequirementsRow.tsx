import type { Requirement } from '@shared/types';
import { StatusBadge, PriorityBadge, TypeBadge } from '../ui/Badge';
import { Avatar } from '../ui/Avatar';
import { formatRelativeDate } from '../../lib/utils';
import { useDetailPanel } from '../../hooks/useDetailPanel';

interface RequirementsRowProps {
  requirement: Requirement;
}

export function RequirementsRow({ requirement }: RequirementsRowProps) {
  const { open } = useDetailPanel();

  return (
    <tr
      onClick={() => open(requirement.spec_id ?? requirement.id)}
      className="border-b border-edge hover:bg-surface-hover/50 transition-colors cursor-pointer group"
    >
      <td className="py-3 px-4">
        <span className="text-[11px] font-mono text-content-muted">{requirement.spec_id}</span>
      </td>
      <td className="py-3 px-4">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium text-content group-hover:text-accent transition-colors">
            {requirement.title}
          </span>
          {requirement.tags && requirement.tags.length > 0 && (
            <div className="flex items-center gap-1 shrink-0">
              {requirement.tags.slice(0, 3).map((tag) => (
                <span key={tag} className="rounded bg-surface-hover px-1.5 py-0.5 text-[10px] text-content-muted">
                  {tag}
                </span>
              ))}
              {requirement.tags.length > 3 && (
                <span className="text-[10px] text-content-muted">+{requirement.tags.length - 3}</span>
              )}
            </div>
          )}
        </div>
      </td>
      <td className="py-3 px-4">
        <StatusBadge status={requirement.status} />
      </td>
      <td className="py-3 px-4">
        <PriorityBadge priority={requirement.priority} />
      </td>
      <td className="py-3 px-4">
        <TypeBadge type={requirement.req_type} />
      </td>
      <td className="py-3 px-4">
        {requirement.owner ? (
          <div className="flex items-center gap-2">
            <Avatar name={requirement.owner} size="sm" />
            <span className="text-xs text-content-secondary">{requirement.owner}</span>
          </div>
        ) : (
          <span className="text-xs text-content-muted">—</span>
        )}
      </td>
      <td className="py-3 px-4 text-right">
        <span className="text-xs text-content-muted">{formatRelativeDate(requirement.modified_at)}</span>
      </td>
    </tr>
  );
}
