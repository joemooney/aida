import { useState, useEffect } from 'react';
import { useMetadata, useUpdateMetadata } from '../../hooks/useSettings';
import { Spinner } from '../ui/Spinner';

export function GeneralTab() {
  const { data, isLoading } = useMetadata();
  const updateMutation = useUpdateMetadata();

  const [name, setName] = useState('');
  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');

  useEffect(() => {
    if (data) {
      setName(data.name);
      setTitle(data.title);
      setDescription(data.description);
    }
  }, [data]);

  if (isLoading) {
    return <div className="flex justify-center py-12"><Spinner /></div>;
  }

  const hasChanges = data && (name !== data.name || title !== data.title || description !== data.description);

  const handleSave = () => {
    updateMutation.mutate({ name, title, description });
  };

  return (
    <div className="max-w-lg flex flex-col gap-5">
      <label className="flex flex-col gap-1">
        <span className="text-xs font-medium text-content-secondary">Store Name</span>
        <input
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none"
          placeholder="e.g., my-project"
        />
      </label>

      <label className="flex flex-col gap-1">
        <span className="text-xs font-medium text-content-secondary">Title</span>
        <input
          type="text"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none"
          placeholder="Project title"
        />
      </label>

      <label className="flex flex-col gap-1">
        <span className="text-xs font-medium text-content-secondary">Description</span>
        <textarea
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          rows={4}
          className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none resize-y"
          placeholder="Describe this project..."
        />
      </label>

      {updateMutation.isError && (
        <p className="text-xs text-red-400">Failed to save. Please try again.</p>
      )}
      {updateMutation.isSuccess && (
        <p className="text-xs text-green-400">Saved successfully.</p>
      )}

      <div>
        <button
          onClick={handleSave}
          disabled={!hasChanges || updateMutation.isPending}
          className="rounded-lg bg-accent px-4 py-2 text-sm font-medium text-white hover:bg-accent/90 disabled:opacity-50 transition-colors"
        >
          {updateMutation.isPending ? 'Saving...' : 'Save'}
        </button>
      </div>
    </div>
  );
}
