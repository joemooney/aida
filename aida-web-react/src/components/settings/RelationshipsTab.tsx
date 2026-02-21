import { useState } from 'react';
import { Plus, Pencil, Trash2 } from 'lucide-react';
import { useRelationshipDefs, useDeleteRelDef } from '../../hooks/useSettings';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { RelationshipForm } from './RelationshipForm';
import type { RelationshipDefinition } from '@shared/types';

export function RelationshipsTab() {
  const { data: defs, isLoading } = useRelationshipDefs();
  const deleteMutation = useDeleteRelDef();
  const [editDef, setEditDef] = useState<RelationshipDefinition | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  if (isLoading) {
    return <div className="flex justify-center py-12"><Spinner /></div>;
  }

  if (!defs?.length) {
    return (
      <EmptyState
        title="No relationship definitions"
        description="Add relationship types to define how requirements connect."
        action={
          <button
            onClick={() => setShowCreate(true)}
            className="rounded-lg bg-accent px-4 py-2 text-sm font-medium text-white hover:bg-accent/90 transition-colors"
          >
            Add Relationship
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
          <Plus className="h-4 w-4" /> Add Relationship
        </button>
      </div>

      <div className="overflow-x-auto rounded-lg border border-edge">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-edge bg-surface-alt">
              <th className="px-4 py-2 text-left font-medium text-content-secondary">Name</th>
              <th className="px-4 py-2 text-left font-medium text-content-secondary">Display Name</th>
              <th className="px-4 py-2 text-left font-medium text-content-secondary">Inverse</th>
              <th className="px-4 py-2 text-center font-medium text-content-secondary">Symmetric</th>
              <th className="px-4 py-2 text-left font-medium text-content-secondary">Cardinality</th>
              <th className="px-4 py-2 text-center font-medium text-content-secondary">Built-in</th>
              <th className="px-4 py-2 text-right font-medium text-content-secondary">Actions</th>
            </tr>
          </thead>
          <tbody>
            {defs.map((def) => (
              <tr key={def.name} className="border-b border-edge last:border-b-0 hover:bg-surface-hover transition-colors">
                <td className="px-4 py-2 font-mono text-content">{def.name}</td>
                <td className="px-4 py-2 text-content">{def.display_name}</td>
                <td className="px-4 py-2 text-content-secondary">{def.inverse ?? '—'}</td>
                <td className="px-4 py-2 text-center text-content-secondary">{def.symmetric ? 'Yes' : 'No'}</td>
                <td className="px-4 py-2 text-content-secondary">{formatCardinality(def.cardinality)}</td>
                <td className="px-4 py-2 text-center">
                  {def.built_in && (
                    <span className="inline-flex items-center rounded-full bg-blue-500/10 px-2 py-0.5 text-[11px] font-medium text-blue-400">
                      built-in
                    </span>
                  )}
                </td>
                <td className="px-4 py-2">
                  <div className="flex items-center justify-end gap-1">
                    <button
                      onClick={() => setEditDef(def)}
                      className="p-1 rounded text-content-muted hover:text-content hover:bg-surface-hover transition-colors"
                      title="Edit"
                    >
                      <Pencil className="h-4 w-4" />
                    </button>
                    {!def.built_in && (
                      confirmDelete === def.name ? (
                        <span className="flex items-center gap-1 text-xs">
                          <button
                            onClick={() => { deleteMutation.mutate(def.name); setConfirmDelete(null); }}
                            className="text-red-400 hover:text-red-300 font-medium"
                          >
                            Confirm
                          </button>
                          <button
                            onClick={() => setConfirmDelete(null)}
                            className="text-content-muted hover:text-content"
                          >
                            Cancel
                          </button>
                        </span>
                      ) : (
                        <button
                          onClick={() => setConfirmDelete(def.name)}
                          className="p-1 rounded text-content-muted hover:text-red-400 hover:bg-surface-hover transition-colors"
                          title="Delete"
                        >
                          <Trash2 className="h-4 w-4" />
                        </button>
                      )
                    )}
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {(showCreate || editDef) && (
        <RelationshipForm
          definition={editDef ?? undefined}
          onClose={() => { setShowCreate(false); setEditDef(null); }}
        />
      )}
    </div>
  );
}

function formatCardinality(c: string): string {
  switch (c) {
    case 'OneToOne': return '1:1';
    case 'OneToMany': return '1:N';
    case 'ManyToOne': return 'N:1';
    case 'ManyToMany': return 'N:N';
    default: return c;
  }
}
