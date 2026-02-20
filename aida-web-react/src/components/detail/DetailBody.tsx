import { useState } from 'react';
import { Plus, X } from 'lucide-react';
import type { Requirement } from '@shared/types';
import { Avatar } from '../ui/Avatar';
import { EditableText } from '../ui/EditableField';
import { formatDate } from '../../lib/utils';
import { useUpdateRequirement } from '../../hooks/useRequirements';

interface DetailBodyProps {
  requirement: Requirement;
}

export function DetailBody({ requirement }: DetailBodyProps) {
  const updateReq = useUpdateRequirement();
  const reqId = requirement.spec_id ?? requirement.id;
  const [newTag, setNewTag] = useState('');

  function save(data: Partial<Requirement>) {
    updateReq.mutate({ id: reqId, data });
  }

  function addTag() {
    const tag = newTag.trim();
    if (!tag) return;
    const current = requirement.tags ?? [];
    if (!current.includes(tag)) {
      save({ tags: [...current, tag] });
    }
    setNewTag('');
  }

  function removeTag(tag: string) {
    const current = requirement.tags ?? [];
    save({ tags: current.filter((t) => t !== tag) });
  }

  return (
    <div className="px-6 py-4 space-y-6 overflow-y-auto flex-1">
      {/* Description */}
      <div>
        <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-2">Description</h3>
        <EditableText
          value={requirement.description}
          onSave={(description) => save({ description })}
          className="text-sm text-content-secondary leading-relaxed whitespace-pre-wrap"
          placeholder="Add a description..."
          multiline
        />
      </div>

      {/* Metadata */}
      <div>
        <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-3">Details</h3>
        <div className="space-y-3">
          {/* Owner — editable */}
          <div className="flex items-center justify-between">
            <span className="text-xs text-content-muted">Owner</span>
            <div className="flex items-center gap-2">
              {requirement.owner && <Avatar name={requirement.owner} size="sm" />}
              <EditableText
                value={requirement.owner}
                onSave={(owner) => save({ owner })}
                className="text-sm text-content"
                placeholder="Assign owner..."
              />
            </div>
          </div>

          {/* Feature — editable */}
          <div className="flex items-center justify-between">
            <span className="text-xs text-content-muted">Feature</span>
            <EditableText
              value={requirement.feature}
              onSave={(feature) => save({ feature })}
              className="text-sm text-content"
              placeholder="Set feature..."
            />
          </div>

          {/* Read-only fields */}
          <div className="flex items-center justify-between">
            <span className="text-xs text-content-muted">Created</span>
            <span className="text-sm text-content">{formatDate(requirement.created_at)}</span>
          </div>
          <div className="flex items-center justify-between">
            <span className="text-xs text-content-muted">Modified</span>
            <span className="text-sm text-content">{formatDate(requirement.modified_at)}</span>
          </div>
        </div>
      </div>

      {/* Tags — editable */}
      <div>
        <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-2">Tags</h3>
        <div className="flex flex-wrap gap-1.5 mb-2">
          {(requirement.tags ?? []).map((tag) => (
            <span key={tag} className="inline-flex items-center gap-1 rounded-md bg-surface-hover pl-2 pr-1 py-0.5 text-xs text-content-secondary group/tag">
              {tag}
              <button
                onClick={() => removeTag(tag)}
                className="rounded-sm p-0.5 text-content-muted hover:text-red-400 opacity-0 group-hover/tag:opacity-100 transition-opacity cursor-pointer"
              >
                <X className="h-2.5 w-2.5" />
              </button>
            </span>
          ))}
        </div>
        <div className="flex items-center gap-1.5">
          <input
            type="text"
            value={newTag}
            onChange={(e) => setNewTag(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && addTag()}
            placeholder="Add tag..."
            className="flex-1 rounded-md border border-edge bg-surface px-2 py-1 text-xs text-content placeholder:text-content-muted focus:border-accent focus:outline-none"
          />
          <button
            onClick={addTag}
            disabled={!newTag.trim()}
            className="flex h-6 w-6 items-center justify-center rounded-md text-content-muted hover:text-accent hover:bg-surface-hover disabled:opacity-30 transition-colors cursor-pointer disabled:cursor-not-allowed"
          >
            <Plus className="h-3.5 w-3.5" />
          </button>
        </div>
      </div>

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
