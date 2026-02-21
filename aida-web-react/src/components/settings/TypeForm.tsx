import { useState, useEffect, useCallback } from 'react';
import { X, Plus, Trash2, ChevronDown, ChevronRight } from 'lucide-react';
import { useCreateTypeDef, useUpdateTypeDef } from '../../hooks/useSettings';
import type { CustomTypeDefinition, CustomFieldDefinition, CustomFieldType } from '@shared/types';

interface TypeFormProps {
  definition?: CustomTypeDefinition;
  onClose: () => void;
}

const FIELD_TYPES: CustomFieldType[] = ['text', 'textarea', 'select', 'boolean', 'date', 'user', 'requirement', 'number'];

export function TypeForm({ definition, onClose }: TypeFormProps) {
  const isEdit = !!definition;
  const createMutation = useCreateTypeDef();
  const updateMutation = useUpdateTypeDef();
  const isPending = createMutation.isPending || updateMutation.isPending;

  const [name, setName] = useState(definition?.name ?? '');
  const [displayName, setDisplayName] = useState(definition?.display_name ?? '');
  const [description, setDescription] = useState(definition?.description ?? '');
  const [prefix, setPrefix] = useState(definition?.prefix ?? '');
  const [color, setColor] = useState(definition?.color ?? '');
  const [stateless, setStateless] = useState(definition?.stateless ?? false);
  const [statuses, setStatuses] = useState<string[]>(definition?.statuses ?? []);
  const [priorities, setPriorities] = useState<string[]>(definition?.priorities ?? []);
  const [customFields, setCustomFields] = useState<CustomFieldDefinition[]>(definition?.custom_fields ?? []);
  const [showFields, setShowFields] = useState(false);
  const [newStatus, setNewStatus] = useState('');
  const [newPriority, setNewPriority] = useState('');

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
    const def: CustomTypeDefinition = {
      name: name.trim(),
      display_name: displayName.trim(),
      description: description.trim() || null,
      prefix: prefix.trim().toUpperCase() || null,
      color: color.trim() || null,
      stateless,
      statuses,
      priorities,
      custom_fields: customFields,
      built_in: definition?.built_in ?? false,
    };

    if (isEdit) {
      updateMutation.mutate({ name: definition!.name, def }, { onSuccess: onClose });
    } else {
      createMutation.mutate(def, { onSuccess: onClose });
    }
  };

  const addField = () => {
    setCustomFields([...customFields, {
      name: '',
      label: '',
      field_type: 'text',
      required: false,
      default_value: null,
      description: null,
      order: customFields.length,
    }]);
  };

  const updateField = (idx: number, partial: Partial<CustomFieldDefinition>) => {
    setCustomFields(customFields.map((f, i) => i === idx ? { ...f, ...partial } : f));
  };

  const removeField = (idx: number) => {
    setCustomFields(customFields.filter((_, i) => i !== idx));
  };

  const isError = createMutation.isError || updateMutation.isError;

  return (
    <>
      <div className="fixed inset-0 bg-black/40 z-40 animate-fade-in" onClick={handleClose} />
      <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
        <form
          onSubmit={handleSubmit}
          className="bg-surface-alt border border-edge rounded-2xl shadow-2xl shadow-black/40 w-full max-w-xl flex flex-col gap-5 p-6 animate-fade-in max-h-[90vh] overflow-y-auto"
          onClick={(e) => e.stopPropagation()}
        >
          <div className="flex items-center justify-between">
            <h2 className="text-lg font-semibold text-content">
              {isEdit ? 'Edit Type' : 'New Type'}
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
                  className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none disabled:opacity-50"
                  placeholder="e.g., ChangeRequest"
                />
              </label>
              <label className="flex flex-col gap-1 flex-1">
                <span className="text-xs font-medium text-content-secondary">Display Name</span>
                <input
                  type="text"
                  required
                  value={displayName}
                  onChange={(e) => setDisplayName(e.target.value)}
                  className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
                  placeholder="e.g., Change Request"
                />
              </label>
            </div>

            <div className="flex gap-3">
              <label className="flex flex-col gap-1 flex-1">
                <span className="text-xs font-medium text-content-secondary">Prefix</span>
                <input
                  type="text"
                  value={prefix}
                  onChange={(e) => setPrefix(e.target.value.toUpperCase())}
                  className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content font-mono focus:border-accent focus:outline-none"
                  placeholder="e.g., CR"
                />
              </label>
              <label className="flex flex-col gap-1 flex-1">
                <span className="text-xs font-medium text-content-secondary">Color</span>
                <input
                  type="text"
                  value={color}
                  onChange={(e) => setColor(e.target.value)}
                  className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
                  placeholder="#4ade80"
                />
              </label>
            </div>

            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-content-secondary">Description</span>
              <textarea
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                rows={2}
                className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none resize-y"
              />
            </label>

            <label className="flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={stateless}
                onChange={(e) => setStateless(e.target.checked)}
                className="rounded border-edge accent-accent"
              />
              <span className="text-sm text-content">Stateless (no status/priority tracking)</span>
            </label>

            {/* Statuses */}
            {!stateless && (
              <div className="flex flex-col gap-2">
                <span className="text-xs font-medium text-content-secondary">Custom Statuses (empty = defaults)</span>
                <div className="flex flex-wrap gap-1">
                  {statuses.map((s, i) => (
                    <span key={i} className="inline-flex items-center gap-1 rounded-full bg-surface px-2.5 py-0.5 text-xs text-content border border-edge">
                      {s}
                      <button type="button" onClick={() => setStatuses(statuses.filter((_, j) => j !== i))} className="text-content-muted hover:text-red-400">
                        <X className="h-3 w-3" />
                      </button>
                    </span>
                  ))}
                </div>
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={newStatus}
                    onChange={(e) => setNewStatus(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') { e.preventDefault(); if (newStatus.trim()) { setStatuses([...statuses, newStatus.trim()]); setNewStatus(''); } }
                    }}
                    className="flex-1 rounded-lg border border-edge bg-surface px-3 py-1.5 text-xs text-content focus:border-accent focus:outline-none"
                    placeholder="Add status..."
                  />
                  <button
                    type="button"
                    onClick={() => { if (newStatus.trim()) { setStatuses([...statuses, newStatus.trim()]); setNewStatus(''); } }}
                    className="text-xs text-accent hover:text-accent/80"
                  >
                    Add
                  </button>
                </div>
              </div>
            )}

            {/* Priorities */}
            {!stateless && (
              <div className="flex flex-col gap-2">
                <span className="text-xs font-medium text-content-secondary">Custom Priorities (empty = defaults)</span>
                <div className="flex flex-wrap gap-1">
                  {priorities.map((p, i) => (
                    <span key={i} className="inline-flex items-center gap-1 rounded-full bg-surface px-2.5 py-0.5 text-xs text-content border border-edge">
                      {p}
                      <button type="button" onClick={() => setPriorities(priorities.filter((_, j) => j !== i))} className="text-content-muted hover:text-red-400">
                        <X className="h-3 w-3" />
                      </button>
                    </span>
                  ))}
                </div>
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={newPriority}
                    onChange={(e) => setNewPriority(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') { e.preventDefault(); if (newPriority.trim()) { setPriorities([...priorities, newPriority.trim()]); setNewPriority(''); } }
                    }}
                    className="flex-1 rounded-lg border border-edge bg-surface px-3 py-1.5 text-xs text-content focus:border-accent focus:outline-none"
                    placeholder="Add priority..."
                  />
                  <button
                    type="button"
                    onClick={() => { if (newPriority.trim()) { setPriorities([...priorities, newPriority.trim()]); setNewPriority(''); } }}
                    className="text-xs text-accent hover:text-accent/80"
                  >
                    Add
                  </button>
                </div>
              </div>
            )}

            {/* Custom fields */}
            <div className="flex flex-col gap-2">
              <button
                type="button"
                onClick={() => setShowFields(!showFields)}
                className="flex items-center gap-1 text-xs font-medium text-content-secondary hover:text-content"
              >
                {showFields ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
                Custom Fields ({customFields.length})
              </button>
              {showFields && (
                <div className="flex flex-col gap-3 pl-2 border-l-2 border-edge">
                  {customFields.map((field, idx) => (
                    <div key={idx} className="flex flex-col gap-2 bg-surface rounded-lg p-3 border border-edge">
                      <div className="flex gap-2">
                        <input
                          type="text"
                          value={field.name}
                          onChange={(e) => updateField(idx, { name: e.target.value })}
                          className="flex-1 rounded border border-edge bg-surface-alt px-2 py-1 text-xs text-content focus:border-accent focus:outline-none"
                          placeholder="Field name"
                        />
                        <input
                          type="text"
                          value={field.label}
                          onChange={(e) => updateField(idx, { label: e.target.value })}
                          className="flex-1 rounded border border-edge bg-surface-alt px-2 py-1 text-xs text-content focus:border-accent focus:outline-none"
                          placeholder="Label"
                        />
                        <select
                          value={field.field_type}
                          onChange={(e) => updateField(idx, { field_type: e.target.value as CustomFieldType })}
                          className="rounded border border-edge bg-surface-alt px-2 py-1 text-xs text-content focus:border-accent focus:outline-none"
                        >
                          {FIELD_TYPES.map((t) => (
                            <option key={t} value={t}>{t}</option>
                          ))}
                        </select>
                        <label className="flex items-center gap-1 text-xs text-content-secondary">
                          <input
                            type="checkbox"
                            checked={field.required}
                            onChange={(e) => updateField(idx, { required: e.target.checked })}
                            className="accent-accent"
                          />
                          Req
                        </label>
                        <button type="button" onClick={() => removeField(idx)} className="p-1 text-content-muted hover:text-red-400">
                          <Trash2 className="h-3.5 w-3.5" />
                        </button>
                      </div>
                    </div>
                  ))}
                  <button
                    type="button"
                    onClick={addField}
                    className="flex items-center gap-1 text-xs text-accent hover:text-accent/80"
                  >
                    <Plus className="h-3.5 w-3.5" /> Add Field
                  </button>
                </div>
              )}
            </div>
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
