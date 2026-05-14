import { useState, useEffect, useCallback } from 'react';
import { X } from 'lucide-react';
import { useCreateRequirement } from '../../hooks/useRequirements';
import type { RequirementType, RequirementStatus, RequirementPriority } from '@shared/types';

interface CreateRequirementModalProps {
  onClose: () => void;
  initialType?: string;
  initialTitle?: string;
}

const TYPE_GROUPS: { label: string; types: { value: RequirementType; label: string }[] }[] = [
  {
    label: 'Agile',
    types: [
      { value: 'Story', label: 'Story' },
      { value: 'Bug', label: 'Bug' },
      { value: 'Task', label: 'Task' },
      { value: 'Epic', label: 'Epic' },
      { value: 'Spike', label: 'Spike' },
    ],
  },
  {
    label: 'Requirements',
    types: [
      { value: 'Functional', label: 'Functional' },
      { value: 'NonFunctional', label: 'Non-Functional' },
      { value: 'System', label: 'System' },
      { value: 'User', label: 'User' },
    ],
  },
  {
    label: 'Organizational',
    types: [
      { value: 'Folder', label: 'Folder' },
    ],
  },
];

const STATUS_OPTIONS: { value: RequirementStatus; label: string }[] = [
  { value: 'Draft', label: 'Draft' },
  { value: 'Approved', label: 'Approved' },
  { value: 'Planned', label: 'Planned' },
  { value: 'InProgress', label: 'In Progress' },
  // STORY-86: Done = work finished on a branch; auto-bumps to
  // Completed once the referencing commit lands on the default branch.
  { value: 'Done', label: 'Done' },
  { value: 'Completed', label: 'Completed' },
  { value: 'Rejected', label: 'Rejected' },
];

const PRIORITY_OPTIONS: { value: RequirementPriority; label: string }[] = [
  { value: 'High', label: 'High' },
  { value: 'Medium', label: 'Medium' },
  { value: 'Low', label: 'Low' },
];

const inputClass = 'rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none';

export function CreateRequirementModal({ onClose, initialType, initialTitle }: CreateRequirementModalProps) {
  const createMutation = useCreateRequirement();
  const [reqType, setReqType] = useState<RequirementType>((initialType as RequirementType) || 'Story');
  const [title, setTitle] = useState(initialTitle ?? '');
  const [description, setDescription] = useState('');
  const [status, setStatus] = useState<RequirementStatus>('Draft');
  const [priority, setPriority] = useState<RequirementPriority>('Medium');
  const [owner, setOwner] = useState('');
  const [tags, setTags] = useState('');

  const handleClose = useCallback(() => {
    if (!createMutation.isPending) onClose();
  }, [createMutation.isPending, onClose]);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') handleClose();
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [handleClose]);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const tagList = tags.split(',').map((t) => t.trim()).filter(Boolean);
    createMutation.mutate(
      {
        title,
        description,
        status,
        priority,
        req_type: reqType,
        owner: owner || undefined,
        tags: tagList.length > 0 ? tagList : undefined,
      } as Parameters<typeof createMutation.mutate>[0],
      { onSuccess: onClose },
    );
  };

  return (
    <>
      {/* Backdrop */}
      <div
        className="fixed inset-0 bg-black/40 z-40 animate-fade-in"
        onClick={handleClose}
      />

      {/* Modal */}
      <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
        <form
          onSubmit={handleSubmit}
          className="bg-surface-alt border border-edge rounded-2xl shadow-2xl shadow-black/40 w-full max-w-lg flex flex-col gap-5 p-6 animate-fade-in"
          onClick={(e) => e.stopPropagation()}
        >
          {/* Header */}
          <div className="flex items-center justify-between">
            <h2 className="text-lg font-semibold text-content">New Requirement</h2>
            <button
              type="button"
              onClick={handleClose}
              className="p-1 rounded-lg text-content-muted hover:text-content hover:bg-surface-hover transition-colors"
            >
              <X className="h-5 w-5" />
            </button>
          </div>

          {/* Fields */}
          <div className="flex flex-col gap-4">
            {/* Type */}
            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-content-secondary">Type</span>
              <select
                value={reqType}
                onChange={(e) => setReqType(e.target.value as RequirementType)}
                className={inputClass}
              >
                {TYPE_GROUPS.map((group) => (
                  <optgroup key={group.label} label={group.label}>
                    {group.types.map((t) => (
                      <option key={t.value} value={t.value}>{t.label}</option>
                    ))}
                  </optgroup>
                ))}
              </select>
            </label>

            {/* Title */}
            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-content-secondary">Title</span>
              <input
                type="text"
                required
                autoFocus
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                placeholder="Requirement title"
                className={inputClass}
              />
            </label>

            {/* Description */}
            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-content-secondary">Description</span>
              <textarea
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="Optional description..."
                rows={3}
                className={`${inputClass} resize-y`}
              />
            </label>

            {/* Status + Priority row */}
            <div className="flex gap-3">
              <label className="flex flex-col gap-1 flex-1">
                <span className="text-xs font-medium text-content-secondary">Status</span>
                <select
                  value={status}
                  onChange={(e) => setStatus(e.target.value as RequirementStatus)}
                  className={inputClass}
                >
                  {STATUS_OPTIONS.map((s) => (
                    <option key={s.value} value={s.value}>{s.label}</option>
                  ))}
                </select>
              </label>
              <label className="flex flex-col gap-1 flex-1">
                <span className="text-xs font-medium text-content-secondary">Priority</span>
                <select
                  value={priority}
                  onChange={(e) => setPriority(e.target.value as RequirementPriority)}
                  className={inputClass}
                >
                  {PRIORITY_OPTIONS.map((p) => (
                    <option key={p.value} value={p.value}>{p.label}</option>
                  ))}
                </select>
              </label>
            </div>

            {/* Owner */}
            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-content-secondary">Owner</span>
              <input
                type="text"
                value={owner}
                onChange={(e) => setOwner(e.target.value)}
                placeholder="Optional owner"
                className={inputClass}
              />
            </label>

            {/* Tags */}
            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-content-secondary">Tags</span>
              <input
                type="text"
                value={tags}
                onChange={(e) => setTags(e.target.value)}
                placeholder="Comma-separated tags"
                className={inputClass}
              />
            </label>
          </div>

          {/* Error */}
          {createMutation.isError && (
            <p className="text-xs text-red-400">
              Failed to create requirement. Please try again.
            </p>
          )}

          {/* Actions */}
          <div className="flex justify-end gap-2 pt-1">
            <button
              type="button"
              onClick={handleClose}
              className="rounded-lg px-4 py-2 text-sm font-medium text-content-secondary hover:bg-surface-hover transition-colors"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={createMutation.isPending}
              className="rounded-lg bg-accent px-4 py-2 text-sm font-medium text-white hover:bg-accent/90 disabled:opacity-50 transition-colors"
            >
              {createMutation.isPending ? 'Creating...' : 'Create'}
            </button>
          </div>
        </form>
      </div>
    </>
  );
}
