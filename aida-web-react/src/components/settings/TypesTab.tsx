import { useState } from 'react';
import { Plus, Pencil, Trash2 } from 'lucide-react';
import { useTypeDefs, useDeleteTypeDef } from '../../hooks/useSettings';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { TypeForm } from './TypeForm';
import type { CustomTypeDefinition } from '@shared/types';

export function TypesTab() {
  const { data: defs, isLoading } = useTypeDefs();
  const deleteMutation = useDeleteTypeDef();
  const [editDef, setEditDef] = useState<CustomTypeDefinition | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  if (isLoading) {
    return <div className="flex justify-center py-12"><Spinner /></div>;
  }

  if (!defs?.length) {
    return (
      <EmptyState
        title="No type definitions"
        description="Add custom requirement types."
        action={
          <button
            onClick={() => setShowCreate(true)}
            className="rounded-lg bg-accent px-4 py-2 text-sm font-medium text-white hover:bg-accent/90 transition-colors"
          >
            Add Type
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
          <Plus className="h-4 w-4" /> Add Type
        </button>
      </div>

      <div className="overflow-x-auto rounded-lg border border-edge">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-edge bg-surface-alt">
              <th className="px-4 py-2 text-left font-medium text-content-secondary">Name</th>
              <th className="px-4 py-2 text-left font-medium text-content-secondary">Display Name</th>
              <th className="px-4 py-2 text-left font-medium text-content-secondary">Prefix</th>
              <th className="px-4 py-2 text-center font-medium text-content-secondary">Stateless</th>
              <th className="px-4 py-2 text-center font-medium text-content-secondary">Fields</th>
              <th className="px-4 py-2 text-center font-medium text-content-secondary">Built-in</th>
              <th className="px-4 py-2 text-right font-medium text-content-secondary">Actions</th>
            </tr>
          </thead>
          <tbody>
            {defs.map((def) => (
              <tr key={def.name} className="border-b border-edge last:border-b-0 hover:bg-surface-hover transition-colors">
                <td className="px-4 py-2 font-mono text-content">
                  <div className="flex items-center gap-2">
                    {def.color && (
                      <span className="inline-block h-3 w-3 rounded-full shrink-0" style={{ backgroundColor: def.color }} />
                    )}
                    {def.name}
                  </div>
                </td>
                <td className="px-4 py-2 text-content">{def.display_name}</td>
                <td className="px-4 py-2 font-mono text-content-secondary">{def.prefix ?? '—'}</td>
                <td className="px-4 py-2 text-center text-content-secondary">{def.stateless ? 'Yes' : 'No'}</td>
                <td className="px-4 py-2 text-center text-content-secondary">{def.custom_fields?.length ?? 0}</td>
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
        <TypeForm
          definition={editDef ?? undefined}
          onClose={() => { setShowCreate(false); setEditDef(null); }}
        />
      )}
    </div>
  );
}
