import type { Requirement } from '@shared/types';
import { Avatar } from '../ui/Avatar';
import { formatDate } from '../../lib/utils';

interface DetailBodyProps {
  requirement: Requirement;
}

export function DetailBody({ requirement }: DetailBodyProps) {
  const metaFields = [
    { label: 'Feature', value: requirement.feature },
    { label: 'Owner', value: requirement.owner },
    { label: 'Created', value: formatDate(requirement.created_at) },
    { label: 'Modified', value: formatDate(requirement.modified_at) },
  ];

  return (
    <div className="px-6 py-4 space-y-6 overflow-y-auto flex-1">
      {/* Description */}
      <div>
        <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-2">Description</h3>
        <div className="text-sm text-content-secondary leading-relaxed whitespace-pre-wrap">
          {requirement.description || 'No description provided.'}
        </div>
      </div>

      {/* Metadata */}
      <div>
        <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-3">Details</h3>
        <div className="space-y-3">
          {metaFields.map(({ label, value }) => (
            <div key={label} className="flex items-center justify-between">
              <span className="text-xs text-content-muted">{label}</span>
              <span className="text-sm text-content">{value || '—'}</span>
            </div>
          ))}
          {requirement.owner && (
            <div className="flex items-center justify-between">
              <span className="text-xs text-content-muted">Avatar</span>
              <Avatar name={requirement.owner} size="md" />
            </div>
          )}
        </div>
      </div>

      {/* Tags */}
      {requirement.tags && requirement.tags.length > 0 && (
        <div>
          <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-2">Tags</h3>
          <div className="flex flex-wrap gap-1.5">
            {requirement.tags.map((tag) => (
              <span key={tag} className="rounded-md bg-surface-hover px-2 py-0.5 text-xs text-content-secondary">
                {tag}
              </span>
            ))}
          </div>
        </div>
      )}

      {/* Dependencies */}
      {requirement.dependencies && requirement.dependencies.length > 0 && (
        <div>
          <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-2">Dependencies</h3>
          <div className="space-y-1">
            {requirement.dependencies.map((dep) => (
              <span key={dep} className="block text-xs font-mono text-accent">{dep}</span>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
