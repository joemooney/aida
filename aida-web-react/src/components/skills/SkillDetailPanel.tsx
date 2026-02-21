import { useEffect, useState, useMemo } from 'react';
import { X, Pencil, Eye, Save } from 'lucide-react';
import { cn } from '../../lib/utils';
import { LinkedMarkdown } from '../ui/LinkedMarkdown';
import { useSkill, useUpdateSkill } from '../../hooks/useSkills';
import { Spinner } from '../ui/Spinner';

interface SkillDetailPanelProps {
  name: string;
  onClose: () => void;
}

export function SkillDetailPanel({ name, onClose }: SkillDetailPanelProps) {
  const { data: skill, isLoading, error } = useSkill(name);
  const updateMutation = useUpdateSkill();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');

  // Sync draft when skill loads or changes
  useEffect(() => {
    if (skill) setDraft(skill.content);
  }, [skill]);

  // Close on Escape
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') {
        if (editing) {
          setEditing(false);
        } else {
          onClose();
        }
      }
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [onClose, editing]);

  // Strip YAML frontmatter for preview
  const markdownBody = useMemo(() => {
    const content = skill?.content ?? '';
    if (content.startsWith('---')) {
      const end = content.indexOf('\n---', 3);
      if (end !== -1) return content.slice(end + 4).trim();
    }
    return content;
  }, [skill?.content]);

  function handleSave() {
    updateMutation.mutate(
      { name, content: draft },
      {
        onSuccess: () => setEditing(false),
      },
    );
  }

  return (
    <>
      {/* Backdrop */}
      <div
        className="fixed inset-0 bg-black/40 z-40 animate-fade-in"
        onClick={onClose}
      />

      {/* Panel */}
      <div className="fixed top-0 right-0 bottom-0 z-50 w-full max-w-2xl bg-surface-alt border-l border-edge flex flex-col animate-slide-in-right shadow-2xl shadow-black/40">
        {isLoading ? (
          <div className="flex items-center justify-center flex-1">
            <Spinner size="lg" />
          </div>
        ) : error || !skill ? (
          <div className="flex flex-col items-center justify-center flex-1 text-content-muted">
            <p className="text-sm">Failed to load skill</p>
            <button
              onClick={onClose}
              className="mt-3 text-xs text-accent hover:text-accent-hover cursor-pointer"
            >
              Close panel
            </button>
          </div>
        ) : (
          <>
            {/* Header */}
            <div className="flex items-center justify-between border-b border-edge px-5 py-4">
              <div className="flex items-center gap-3 min-w-0">
                <h2 className="text-base font-semibold text-content truncate">{skill.name}</h2>
                <span
                  className={cn(
                    'shrink-0 inline-flex items-center rounded-md px-2 py-0.5 text-[11px] font-medium',
                    skill.kind === 'skill'
                      ? 'bg-accent/10 text-accent'
                      : 'bg-amber-500/10 text-amber-400',
                  )}
                >
                  {skill.kind}
                </span>
              </div>
              <div className="flex items-center gap-2">
                {editing ? (
                  <button
                    onClick={handleSave}
                    disabled={updateMutation.isPending}
                    className="inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium bg-accent text-white hover:bg-accent-hover transition-colors cursor-pointer disabled:opacity-50"
                  >
                    <Save className="h-3.5 w-3.5" />
                    {updateMutation.isPending ? 'Saving...' : 'Save'}
                  </button>
                ) : null}
                <button
                  onClick={() => setEditing((e) => !e)}
                  className="inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium text-content-secondary hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
                >
                  {editing ? (
                    <>
                      <Eye className="h-3.5 w-3.5" />
                      View
                    </>
                  ) : (
                    <>
                      <Pencil className="h-3.5 w-3.5" />
                      Edit
                    </>
                  )}
                </button>
                <button
                  onClick={onClose}
                  className="rounded-lg p-1.5 text-content-muted hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
                >
                  <X className="h-4 w-4" />
                </button>
              </div>
            </div>

            {/* Tool badges */}
            {skill.allowed_tools.length > 0 && (
              <div className="flex items-center gap-2 px-5 py-2.5 border-b border-edge">
                <span className="text-[11px] font-medium text-content-muted uppercase tracking-wider">Tools:</span>
                <div className="flex flex-wrap gap-1.5">
                  {skill.allowed_tools.map((tool) => (
                    <span
                      key={tool}
                      className="inline-flex items-center rounded-md bg-surface-hover px-2 py-0.5 text-[11px] font-medium text-content-secondary"
                    >
                      {tool}
                    </span>
                  ))}
                </div>
              </div>
            )}

            {/* Content */}
            <div className="flex-1 overflow-y-auto">
              {editing ? (
                <textarea
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  className="w-full h-full p-5 bg-transparent text-sm text-content font-mono resize-none focus:outline-none"
                  spellCheck={false}
                />
              ) : (
                <LinkedMarkdown className="p-5 prose prose-sm prose-invert max-w-none text-content prose-headings:text-content prose-strong:text-content prose-code:text-accent prose-code:bg-surface-hover prose-code:px-1.5 prose-code:py-0.5 prose-code:rounded prose-code:text-xs prose-code:before:content-none prose-code:after:content-none prose-pre:bg-surface-hover prose-pre:border prose-pre:border-edge prose-a:text-accent">
                  {markdownBody}
                </LinkedMarkdown>
              )}
            </div>
          </>
        )}
      </div>
    </>
  );
}
