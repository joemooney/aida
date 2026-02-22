import { useState, useRef, useEffect } from 'react';
import { cn } from '../../lib/utils';
import { useCreateRequirement } from '../../hooks/useRequirements';
import type { RequirementType } from '@shared/types';

const QUICK_TYPES: { value: RequirementType; label: string }[] = [
  { value: 'Story', label: 'Story' },
  { value: 'Bug', label: 'Bug' },
  { value: 'Task', label: 'Task' },
];

interface QuickCreateDropdownProps {
  onClose: () => void;
  onMoreOptions: (type: RequirementType, title: string) => void;
}

export function QuickCreateDropdown({ onClose, onMoreOptions }: QuickCreateDropdownProps) {
  const createMutation = useCreateRequirement();
  const [selectedType, setSelectedType] = useState<RequirementType>('Story');
  const [title, setTitle] = useState('');
  const containerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // Auto-focus title input on mount
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // Click-outside to close
  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        onClose();
      }
    }
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [onClose]);

  // Escape to close
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') onClose();
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [onClose]);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!title.trim()) return;
    createMutation.mutate(
      {
        title: title.trim(),
        req_type: selectedType,
        status: 'Draft',
        priority: 'Medium',
      } as Parameters<typeof createMutation.mutate>[0],
      {
        onSuccess: () => {
          setTitle('');
          onClose();
        },
      },
    );
  };

  return (
    <div
      ref={containerRef}
      className="absolute right-0 top-full mt-2 w-80 rounded-xl border border-edge bg-surface-alt shadow-xl shadow-black/20 z-50 animate-fade-in"
    >
      <form onSubmit={handleSubmit} className="flex flex-col gap-3 p-4">
        {/* Type pills */}
        <div className="flex gap-1.5">
          {QUICK_TYPES.map((t) => (
            <button
              key={t.value}
              type="button"
              onClick={() => setSelectedType(t.value)}
              className={cn(
                'rounded-lg px-3 py-1.5 text-xs font-medium transition-colors',
                selectedType === t.value
                  ? 'bg-accent text-white'
                  : 'bg-surface text-content-secondary hover:bg-surface-hover',
              )}
            >
              {t.label}
            </button>
          ))}
        </div>

        {/* Title input */}
        <input
          ref={inputRef}
          type="text"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          placeholder="Requirement title"
          className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder:text-content-muted focus:border-accent focus:outline-none"
        />

        {/* Error */}
        {createMutation.isError && (
          <p className="text-xs text-red-400">Failed to create. Try again.</p>
        )}

        {/* Actions */}
        <div className="flex items-center justify-between">
          <button
            type="button"
            onClick={() => onMoreOptions(selectedType, title)}
            className="text-xs text-accent hover:text-accent/80 transition-colors"
          >
            More options...
          </button>
          <button
            type="submit"
            disabled={createMutation.isPending || !title.trim()}
            className="rounded-lg bg-accent px-4 py-1.5 text-xs font-medium text-white hover:bg-accent/90 disabled:opacity-50 transition-colors"
          >
            {createMutation.isPending ? 'Creating...' : 'Create'}
          </button>
        </div>
      </form>
    </div>
  );
}
