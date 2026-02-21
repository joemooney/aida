// trace:TASK-014 | ai:claude
import { ChevronRight, ChevronDown } from 'lucide-react';
import type { TreeNode } from '../../lib/tree-utils';
import { StatusBadge, PriorityBadge, TypeBadge } from '../ui/Badge';
import { Avatar } from '../ui/Avatar';
import { formatRelativeDate, cn } from '../../lib/utils';
import { useDetailPanel } from '../../hooks/useDetailPanel';

interface TreeRowProps {
  node: TreeNode;
  isCollapsed: boolean;
  onToggle: (id: string) => void;
  isDimmed: boolean;
}

export function TreeRow({ node, isCollapsed, onToggle, isDimmed }: TreeRowProps) {
  const { open } = useDetailPanel();
  const { requirement, children, depth } = node;
  const hasChildren = children.length > 0;

  return (
    <tr
      onClick={() => open(requirement.spec_id ?? requirement.id)}
      className={cn(
        'border-b border-edge hover:bg-surface-hover/50 transition-colors cursor-pointer group',
        isDimmed && 'opacity-50',
      )}
    >
      <td className="py-3 px-4">
        <span className="text-[11px] font-mono text-content-muted">{requirement.spec_id}</span>
      </td>
      <td className="py-3 px-4">
        <div
          className="flex items-center gap-1"
          style={{ paddingLeft: `${depth * 20}px` }}
        >
          {hasChildren ? (
            <button
              onClick={(e) => {
                e.stopPropagation();
                onToggle(requirement.id);
              }}
              className="p-0.5 rounded hover:bg-surface-hover text-content-muted hover:text-content transition-colors shrink-0"
            >
              {isCollapsed ? (
                <ChevronRight className="h-3.5 w-3.5" />
              ) : (
                <ChevronDown className="h-3.5 w-3.5" />
              )}
            </button>
          ) : (
            <span className="w-[18px] shrink-0" />
          )}
          <span className="text-sm font-medium text-content group-hover:text-accent transition-colors">
            {requirement.title}
          </span>
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
