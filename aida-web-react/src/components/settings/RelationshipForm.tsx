import { useState, useEffect, useCallback } from 'react';
import { X } from 'lucide-react';
import { useCreateRelDef, useUpdateRelDef } from '../../hooks/useSettings';
import type { RelationshipDefinition, Cardinality } from '@shared/types';

interface RelationshipFormProps {
  definition?: RelationshipDefinition;
  onClose: () => void;
}

export function RelationshipForm({ definition, onClose }: RelationshipFormProps) {
  const isEdit = !!definition;
  const createMutation = useCreateRelDef();
  const updateMutation = useUpdateRelDef();
  const isPending = createMutation.isPending || updateMutation.isPending;

  const [name, setName] = useState(definition?.name ?? '');
  const [displayName, setDisplayName] = useState(definition?.display_name ?? '');
  const [description, setDescription] = useState(definition?.description ?? '');
  const [inverse, setInverse] = useState(definition?.inverse ?? '');
  const [symmetric, setSymmetric] = useState(definition?.symmetric ?? false);
  const [cardinality, setCardinality] = useState<Cardinality>(definition?.cardinality ?? 'ManyToMany');
  const [sourceTypes, setSourceTypes] = useState(definition?.source_types.join(', ') ?? '');
  const [targetTypes, setTargetTypes] = useState(definition?.target_types.join(', ') ?? '');
  const [color, setColor] = useState(definition?.color ?? '');

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
    const def: RelationshipDefinition = {
      name: name.toLowerCase().trim(),
      display_name: displayName.trim(),
      description: description.trim(),
      inverse: inverse.trim() || null,
      symmetric,
      cardinality,
      source_types: sourceTypes ? sourceTypes.split(',').map((s) => s.trim()).filter(Boolean) : [],
      target_types: targetTypes ? targetTypes.split(',').map((s) => s.trim()).filter(Boolean) : [],
      built_in: definition?.built_in ?? false,
      color: color.trim() || null,
      icon: definition?.icon ?? null,
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
          className="bg-surface-alt border border-edge rounded-2xl shadow-2xl shadow-black/40 w-full max-w-lg flex flex-col gap-5 p-6 animate-fade-in max-h-[90vh] overflow-y-auto"
          onClick={(e) => e.stopPropagation()}
        >
          <div className="flex items-center justify-between">
            <h2 className="text-lg font-semibold text-content">
              {isEdit ? 'Edit Relationship' : 'New Relationship'}
            </h2>
            <button type="button" onClick={handleClose} className="p-1 rounded-lg text-content-muted hover:text-content hover:bg-surface-hover transition-colors">
              <X className="h-5 w-5" />
            </button>
          </div>

          <div className="flex flex-col gap-4">
            <div className="flex gap-3">
              <label className="flex flex-col gap-1 flex-1">
                <span className="text-xs font-medium text-content-secondary">Name</span>
                <input
                  type="text"
                  required
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  disabled={isEdit && definition?.built_in}
                  className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none disabled:opacity-50"
                  placeholder="e.g., blocks"
                />
              </label>
              <label className="flex flex-col gap-1 flex-1">
                <span className="text-xs font-medium text-content-secondary">Display Name</span>
                <input
                  type="text"
                  required
                  value={displayName}
                  onChange={(e) => setDisplayName(e.target.value)}
                  className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none"
                  placeholder="e.g., Blocks"
                />
              </label>
            </div>

            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-content-secondary">Description</span>
              <textarea
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                rows={2}
                className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none resize-y"
                placeholder="What this relationship means..."
              />
            </label>

            <div className="flex gap-3">
              <label className="flex flex-col gap-1 flex-1">
                <span className="text-xs font-medium text-content-secondary">Inverse</span>
                <input
                  type="text"
                  value={inverse}
                  onChange={(e) => setInverse(e.target.value)}
                  disabled={isEdit && definition?.built_in}
                  className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none disabled:opacity-50"
                  placeholder="e.g., blocked_by"
                />
              </label>
              <label className="flex flex-col gap-1 flex-1">
                <span className="text-xs font-medium text-content-secondary">Cardinality</span>
                <select
                  value={cardinality}
                  onChange={(e) => setCardinality(e.target.value as Cardinality)}
                  disabled={isEdit && definition?.built_in}
                  className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none disabled:opacity-50"
                >
                  <option value="ManyToMany">N:N (Many to Many)</option>
                  <option value="OneToMany">1:N (One to Many)</option>
                  <option value="ManyToOne">N:1 (Many to One)</option>
                  <option value="OneToOne">1:1 (One to One)</option>
                </select>
              </label>
            </div>

            <div className="flex gap-3 items-center">
              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={symmetric}
                  onChange={(e) => setSymmetric(e.target.checked)}
                  disabled={isEdit && definition?.built_in}
                  className="rounded border-edge accent-accent"
                />
                <span className="text-sm text-content">Symmetric</span>
              </label>
              <label className="flex flex-col gap-1 flex-1">
                <span className="text-xs font-medium text-content-secondary">Color</span>
                <input
                  type="text"
                  value={color}
                  onChange={(e) => setColor(e.target.value)}
                  className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none"
                  placeholder="#ff6b6b"
                />
              </label>
            </div>

            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-content-secondary">Source Types (comma-separated, empty = all)</span>
              <input
                type="text"
                value={sourceTypes}
                onChange={(e) => setSourceTypes(e.target.value)}
                className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none"
                placeholder="e.g., functional, bug"
              />
            </label>

            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-content-secondary">Target Types (comma-separated, empty = all)</span>
              <input
                type="text"
                value={targetTypes}
                onChange={(e) => setTargetTypes(e.target.value)}
                className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none"
                placeholder="e.g., functional, bug"
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
