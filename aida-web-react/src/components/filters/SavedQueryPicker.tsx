import { useState, useRef, useEffect } from 'react';
import { Save, ChevronDown, Trash2, FolderOpen } from 'lucide-react';
import type { SavedQuery } from '../../hooks/useAdvancedQuery';
import { cn } from '../../lib/utils';

interface SavedQueryPickerProps {
  savedQueries: SavedQuery[];
  onSave: (name: string) => void;
  onLoad: (name: string) => void;
  onDelete: (name: string) => void;
  hasActiveQuery: boolean;
}

export function SavedQueryPicker({ savedQueries, onSave, onLoad, onDelete, hasActiveQuery }: SavedQueryPickerProps) {
  const [isOpen, setIsOpen] = useState(false);
  const [saveName, setSaveName] = useState('');
  const dropdownRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function handleClickOutside(e: MouseEvent) {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setIsOpen(false);
      }
    }
    if (isOpen) {
      document.addEventListener('mousedown', handleClickOutside);
      return () => document.removeEventListener('mousedown', handleClickOutside);
    }
  }, [isOpen]);

  const handleSave = () => {
    const trimmed = saveName.trim();
    if (!trimmed) return;
    onSave(trimmed);
    setSaveName('');
  };

  return (
    <div className="relative" ref={dropdownRef}>
      <button
        onClick={() => setIsOpen(!isOpen)}
        className="flex items-center gap-1.5 rounded-lg border border-edge bg-surface px-2.5 py-1.5 text-xs text-content-muted hover:text-content hover:bg-surface-hover transition-colors"
      >
        <FolderOpen className="h-3.5 w-3.5" />
        Saved
        {savedQueries.length > 0 && (
          <span className="rounded-full bg-accent/15 text-accent px-1.5 text-[10px] font-medium">
            {savedQueries.length}
          </span>
        )}
        <ChevronDown className={cn('h-3 w-3 transition-transform', isOpen && 'rotate-180')} />
      </button>

      {isOpen && (
        <div className="absolute right-0 top-full z-50 mt-1 w-64 rounded-lg border border-edge bg-surface-raised shadow-xl shadow-black/30">
          {/* Save current query */}
          {hasActiveQuery && (
            <div className="border-b border-edge p-2">
              <div className="flex items-center gap-1.5">
                <input
                  type="text"
                  placeholder="Query name..."
                  value={saveName}
                  onChange={(e) => setSaveName(e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && handleSave()}
                  className="flex-1 rounded border border-edge bg-surface px-2 py-1 text-xs text-content placeholder:text-content-muted focus:border-accent focus:outline-none"
                />
                <button
                  onClick={handleSave}
                  disabled={!saveName.trim()}
                  className="flex items-center gap-1 rounded bg-accent px-2 py-1 text-xs text-white hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                >
                  <Save className="h-3 w-3" />
                  Save
                </button>
              </div>
            </div>
          )}

          {/* Saved queries list */}
          {savedQueries.length > 0 ? (
            <div className="max-h-48 overflow-y-auto p-1">
              {savedQueries.map((sq) => (
                <div
                  key={sq.name}
                  className="flex items-center justify-between gap-2 rounded px-2 py-1.5 hover:bg-surface-hover group"
                >
                  <button
                    onClick={() => {
                      onLoad(sq.name);
                      setIsOpen(false);
                    }}
                    className="flex-1 text-left text-xs text-content truncate"
                  >
                    {sq.name}
                  </button>
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      onDelete(sq.name);
                    }}
                    className="shrink-0 rounded p-0.5 text-content-muted opacity-0 group-hover:opacity-100 hover:text-red-400 hover:bg-red-400/10 transition-all"
                    title="Delete query"
                  >
                    <Trash2 className="h-3 w-3" />
                  </button>
                </div>
              ))}
            </div>
          ) : (
            <div className="px-3 py-4 text-center text-xs text-content-muted">
              No saved queries yet
            </div>
          )}
        </div>
      )}
    </div>
  );
}
