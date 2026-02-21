import { useState, useEffect, useCallback } from 'react';
import { X } from 'lucide-react';
import { useCreateReactionDef, useUpdateReactionDef } from '../../hooks/useSettings';
import type { ReactionDefinition } from '@shared/types';

interface ReactionFormProps {
  definition?: ReactionDefinition;
  onClose: () => void;
}

export function ReactionForm({ definition, onClose }: ReactionFormProps) {
  const isEdit = !!definition;
  const createMutation = useCreateReactionDef();
  const updateMutation = useUpdateReactionDef();
  const isPending = createMutation.isPending || updateMutation.isPending;

  const [name, setName] = useState(definition?.name ?? '');
  const [emoji, setEmoji] = useState(definition?.emoji ?? '');
  const [label, setLabel] = useState(definition?.label ?? '');
  const [description, setDescription] = useState(definition?.description ?? '');

  const handleClose = useCallback(() => {
    if (!isPending) onClose();
  }, [isPending, onClose]);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') handleClose();
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [handleClose]);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const def: ReactionDefinition = {
      name: name.trim(),
      emoji: emoji.trim(),
      label: label.trim(),
      description: description.trim() || null,
      built_in: definition?.built_in ?? false,
    };

    if (isEdit) {
      updateMutation.mutate({ name: definition!.name, def }, { onSuccess: onClose });
    } else {
      createMutation.mutate(def, { onSuccess: onClose });
    }
  };

  const isError = createMutation.isError || updateMutation.isError;

  return (
    <>
      <div className="fixed inset-0 bg-black/40 z-40 animate-fade-in" onClick={handleClose} />
      <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
        <form
          onSubmit={handleSubmit}
          className="bg-surface-alt border border-edge rounded-2xl shadow-2xl shadow-black/40 w-full max-w-sm flex flex-col gap-5 p-6 animate-fade-in"
          onClick={(e) => e.stopPropagation()}
        >
          <div className="flex items-center justify-between">
            <h2 className="text-lg font-semibold text-content">
              {isEdit ? 'Edit Reaction' : 'New Reaction'}
            </h2>
            <button type="button" onClick={handleClose} className="p-1 rounded-lg text-content-muted hover:text-content hover:bg-surface-hover transition-colors">
              <X className="h-5 w-5" />
            </button>
          </div>

          <div className="flex flex-col gap-4">
            <div className="flex gap-3">
              <label className="flex flex-col gap-1 w-20">
                <span className="text-xs font-medium text-content-secondary">Emoji</span>
                <input
                  type="text"
                  required
                  value={emoji}
                  onChange={(e) => setEmoji(e.target.value)}
                  className="rounded-lg border border-edge bg-surface px-3 py-2 text-center text-xl focus:border-accent focus:outline-none"
                />
              </label>
              <label className="flex flex-col gap-1 flex-1">
                <span className="text-xs font-medium text-content-secondary">Name</span>
                <input
                  type="text"
                  required
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  disabled={isEdit && definition?.built_in}
                  className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none disabled:opacity-50"
                  placeholder="e.g., thumbs_up"
                />
              </label>
            </div>

            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-content-secondary">Label</span>
              <input
                type="text"
                required
                value={label}
                onChange={(e) => setLabel(e.target.value)}
                className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
                placeholder="e.g., Thumbs Up"
              />
            </label>

            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-content-secondary">Description</span>
              <input
                type="text"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
                placeholder="When to use this reaction..."
              />
            </label>
          </div>

          {isError && (
            <p className="text-xs text-red-400">Failed to save. Please try again.</p>
          )}

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
              disabled={isPending}
              className="rounded-lg bg-accent px-4 py-2 text-sm font-medium text-white hover:bg-accent/90 disabled:opacity-50 transition-colors"
            >
              {isPending ? 'Saving...' : isEdit ? 'Update' : 'Create'}
            </button>
          </div>
        </form>
      </div>
    </>
  );
}
