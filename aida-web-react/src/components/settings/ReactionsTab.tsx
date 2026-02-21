import { useState } from 'react';
import { Plus, Pencil, Trash2 } from 'lucide-react';
import { useReactionDefs, useDeleteReactionDef } from '../../hooks/useSettings';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { ReactionForm } from './ReactionForm';
import type { ReactionDefinition } from '@shared/types';

export function ReactionsTab() {
  const { data: defs, isLoading } = useReactionDefs();
  const deleteMutation = useDeleteReactionDef();
  const [editDef, setEditDef] = useState<ReactionDefinition | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  if (isLoading) {
    return <div className="flex justify-center py-12"><Spinner /></div>;
  }

  if (!defs?.length) {
    return (
      <EmptyState
        title="No reaction definitions"
        description="Add emoji reactions for comments."
        action={
          <button
            onClick={() => setShowCreate(true)}
            className="rounded-lg bg-accent px-4 py-2 text-sm font-medium text-white hover:bg-accent/90 transition-colors"
          >
            Add Reaction
          </button>
        }
      />
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex justify-end">
        <button
          onClick={() => setShowCreate(true)}
          className="flex items-center gap-1.5 rounded-lg bg-accent px-3 py-1.5 text-sm font-medium text-white hover:bg-accent/90 transition-colors"
        >
          <Plus className="h-4 w-4" /> Add Reaction
        </button>
      </div>

      <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-3">
        {defs.map((def) => (
          <div
            key={def.name}
            className="flex flex-col items-center gap-2 rounded-xl border border-edge bg-surface p-4 hover:bg-surface-hover transition-colors relative group"
          >
            <span className="text-3xl">{def.emoji}</span>
            <span className="text-sm font-medium text-content">{def.label}</span>
            <span className="text-xs font-mono text-content-muted">{def.name}</span>
            {def.built_in && (
              <span className="inline-flex items-center rounded-full bg-blue-500/10 px-2 py-0.5 text-[11px] font-medium text-blue-400">
                built-in
              </span>
            )}
            {def.description && (
              <p className="text-[11px] text-content-secondary text-center">{def.description}</p>
            )}

            {/* Actions overlay */}
            <div className="absolute top-2 right-2 flex gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity">
              <button
                onClick={() => setEditDef(def)}
                className="p-1 rounded text-content-muted hover:text-content hover:bg-surface-alt transition-colors"
                title="Edit"
              >
                <Pencil className="h-3.5 w-3.5" />
              </button>
              {!def.built_in && (
                confirmDelete === def.name ? (
                  <span className="flex items-center gap-1 text-[10px]">
                    <button
                      onClick={() => { deleteMutation.mutate(def.name); setConfirmDelete(null); }}
                      className="text-red-400 hover:text-red-300 font-medium"
                    >
                      Yes
                    </button>
                    <button
                      onClick={() => setConfirmDelete(null)}
                      className="text-content-muted hover:text-content"
                    >
                      No
                    </button>
                  </span>
                ) : (
                  <button
                    onClick={() => setConfirmDelete(def.name)}
                    className="p-1 rounded text-content-muted hover:text-red-400 hover:bg-surface-alt transition-colors"
                    title="Delete"
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </button>
                )
              )}
            </div>
          </div>
        ))}
      </div>

      {(showCreate || editDef) && (
        <ReactionForm
          definition={editDef ?? undefined}
          onClose={() => { setShowCreate(false); setEditDef(null); }}
        />
      )}
    </div>
  );
}
