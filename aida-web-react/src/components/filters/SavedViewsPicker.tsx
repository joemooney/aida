import { useEffect, useMemo, useRef, useState } from 'react';
import { Bookmark, ChevronDown, Pencil, Trash2 } from 'lucide-react';
import type { ListViewMode, SavedView, SavedViewPage } from '../../hooks/useSavedViews';

export interface SavedViewSettingsPatch {
  isDefault?: boolean;
  showInSidebar?: boolean;
  showFilterBar?: boolean;
  listViewMode?: ListViewMode;
}

interface SavedViewsPickerProps {
  page: SavedViewPage;
  views: SavedView[];
  onLoad: (view: SavedView) => void;
  onDelete: (id: string) => void;
  onUpdateSettings: (id: string, patch: SavedViewSettingsPatch) => void;
}

export function SavedViewsPicker({
  page,
  views,
  onLoad,
  onDelete,
  onUpdateSettings,
}: SavedViewsPickerProps) {
  const [isOpen, setIsOpen] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [settings, setSettings] = useState<SavedViewSettingsPatch>({});
  const dropdownRef = useRef<HTMLDivElement>(null);

  const isListPage = page === 'list';

  useEffect(() => {
    function handleClickOutside(e: MouseEvent) {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setIsOpen(false);
        setEditingId(null);
      }
    }
    if (isOpen) {
      document.addEventListener('mousedown', handleClickOutside);
      return () => document.removeEventListener('mousedown', handleClickOutside);
    }
  }, [isOpen]);

  const sortedViews = useMemo(
    () => [...views].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt)),
    [views],
  );

  const startEdit = (view: SavedView) => {
    setEditingId(view.id);
    setSettings({
      isDefault: view.isDefault,
      showInSidebar: view.showInSidebar,
      showFilterBar: view.showFilterBar,
      listViewMode: view.listViewMode,
    });
  };

  return (
    <div className="relative" ref={dropdownRef}>
      <button
        onClick={() => setIsOpen((prev) => !prev)}
        className="flex items-center gap-1.5 rounded-lg border border-edge bg-surface px-2.5 py-1.5 text-xs text-content-muted hover:text-content hover:bg-surface-hover transition-colors"
      >
        <Bookmark className="h-3.5 w-3.5" />
        Saved Views
        {sortedViews.length > 0 && (
          <span className="rounded-full bg-accent/15 text-accent px-1.5 text-[10px] font-medium">
            {sortedViews.length}
          </span>
        )}
        <ChevronDown className={`h-3 w-3 transition-transform ${isOpen ? 'rotate-180' : ''}`} />
      </button>

      {isOpen && (
        <div className="absolute right-0 top-full z-50 mt-1 w-80 rounded-lg border border-edge bg-surface-raised shadow-xl shadow-black/30">
          {sortedViews.length > 0 ? (
            <div className="max-h-80 overflow-y-auto p-1">
              {sortedViews.map((view) => (
                <div key={view.id} className="rounded px-2 py-1.5 hover:bg-surface-hover">
                  <div className="flex items-center gap-1">
                    <button
                      onClick={() => {
                        onLoad(view);
                        setIsOpen(false);
                        setEditingId(null);
                      }}
                      className="flex-1 text-left min-w-0"
                    >
                      <div className="truncate text-xs text-content">{view.name}</div>
                      <div className="flex items-center gap-1 text-[10px] text-content-muted">
                        {view.isDefault ? <span>Default</span> : null}
                        {view.showInSidebar ? <span>Pinned</span> : null}
                        {!view.showFilterBar ? <span>Filters hidden</span> : null}
                        {view.listViewMode ? <span>{view.listViewMode}</span> : null}
                      </div>
                    </button>
                    <button
                      onClick={() => startEdit(view)}
                      className="rounded p-1 text-content-muted hover:text-content hover:bg-surface transition-colors"
                      title="Edit settings"
                    >
                      <Pencil className="h-3.5 w-3.5" />
                    </button>
                    <button
                      onClick={() => onDelete(view.id)}
                      className="rounded p-1 text-content-muted hover:text-red-400 hover:bg-red-500/10 transition-colors"
                      title="Delete view"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </button>
                  </div>

                  {editingId === view.id && (
                    <div className="mt-2 rounded border border-edge bg-surface p-2 space-y-2 text-xs">
                      <label className="inline-flex items-center gap-1.5 cursor-pointer mr-3">
                        <input
                          type="checkbox"
                          checked={!!settings.isDefault}
                          onChange={(e) =>
                            setSettings((prev) => ({ ...prev, isDefault: e.target.checked }))
                          }
                          className="accent-accent"
                        />
                        Default for me
                      </label>
                      <label className="inline-flex items-center gap-1.5 cursor-pointer mr-3">
                        <input
                          type="checkbox"
                          checked={!!settings.showInSidebar}
                          onChange={(e) =>
                            setSettings((prev) => ({ ...prev, showInSidebar: e.target.checked }))
                          }
                          className="accent-accent"
                        />
                        Show in sidebar
                      </label>
                      <label className="inline-flex items-center gap-1.5 cursor-pointer">
                        <input
                          type="checkbox"
                          checked={!!settings.showFilterBar}
                          onChange={(e) =>
                            setSettings((prev) => ({ ...prev, showFilterBar: e.target.checked }))
                          }
                          className="accent-accent"
                        />
                        Show filters
                      </label>
                      {isListPage && (
                        <div className="flex items-center gap-2">
                          <span className="text-content-muted">Mode:</span>
                          <select
                            value={settings.listViewMode ?? 'flat'}
                            onChange={(e) =>
                              setSettings((prev) => ({
                                ...prev,
                                listViewMode: e.target.value as ListViewMode,
                              }))
                            }
                            className="rounded border border-edge bg-surface px-2 py-1 text-xs text-content focus:border-accent focus:outline-none"
                          >
                            <option value="flat">List</option>
                            <option value="tree">Tree</option>
                          </select>
                        </div>
                      )}
                      <div className="flex items-center justify-end gap-2">
                        <button
                          onClick={() => setEditingId(null)}
                          className="rounded border border-edge px-2 py-1 text-xs text-content-muted hover:text-content hover:bg-surface-hover transition-colors"
                        >
                          Cancel
                        </button>
                        <button
                          onClick={() => {
                            onUpdateSettings(view.id, settings);
                            setEditingId(null);
                          }}
                          className="rounded bg-accent px-2 py-1 text-xs font-medium text-white hover:bg-accent/90 transition-colors"
                        >
                          Save
                        </button>
                      </div>
                    </div>
                  )}
                </div>
              ))}
            </div>
          ) : (
            <div className="px-3 py-4 text-center text-xs text-content-muted">
              No saved views yet
            </div>
          )}
        </div>
      )}
    </div>
  );
}

