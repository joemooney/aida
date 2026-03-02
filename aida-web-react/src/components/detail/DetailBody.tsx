import { useState, useRef, useEffect } from 'react';
import { Plus, X, Pencil, Check, Maximize2, Minimize2, Eye, EyeOff, HelpCircle } from 'lucide-react';
import type { Requirement } from '@shared/types';
import { LinkedMarkdown } from '../ui/LinkedMarkdown';
import { Avatar } from '../ui/Avatar';
import { EditableText } from '../ui/EditableField';
import { formatDate, cn } from '../../lib/utils';
import { useUpdateRequirement } from '../../hooks/useRequirements';
import { usePermissions } from '../../hooks/usePermissions';

interface DetailBodyProps {
  requirement: Requirement;
  autoEditDescription?: boolean;
}

export function DetailBody({ requirement, autoEditDescription = false }: DetailBodyProps) {
  const { canWrite } = usePermissions();
  const updateReq = useUpdateRequirement();
  const reqId = requirement.spec_id ?? requirement.id;
  const [newTag, setNewTag] = useState('');
  const [editingDesc, setEditingDesc] = useState(false);
  const [descDraft, setDescDraft] = useState(requirement.description);
  const descRef = useRef<HTMLTextAreaElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [showPreview, setShowPreview] = useState(false);
  const [showHelp, setShowHelp] = useState(false);

  // Sync draft when requirement changes externally
  useEffect(() => {
    if (!editingDesc) setDescDraft(requirement.description);
  }, [requirement.description, editingDesc]);

  useEffect(() => {
    if (editingDesc && descRef.current) {
      descRef.current.focus();
      const len = descRef.current.value.length;
      descRef.current.setSelectionRange(len, len);
    }
  }, [editingDesc]);

  useEffect(() => {
    if (autoEditDescription && canWrite) {
      setEditingDesc(true);
    }
  }, [autoEditDescription, canWrite, requirement.id]);

  function saveDesc() {
    const trimmed = descDraft.trim();
    if (trimmed && trimmed !== requirement.description) {
      save({ description: trimmed });
    } else {
      setDescDraft(requirement.description);
    }
    setEditingDesc(false);
  }

  function cancelDesc() {
    setDescDraft(requirement.description);
    setEditingDesc(false);
  }

  function save(data: Partial<Requirement>) {
    updateReq.mutate({ id: reqId, data });
  }

  function addTag() {
    const tag = newTag.trim();
    if (!tag) return;
    const current = requirement.tags ?? [];
    if (!current.includes(tag)) {
      save({ tags: [...current, tag] });
    }
    setNewTag('');
  }

  function removeTag(tag: string) {
    const current = requirement.tags ?? [];
    save({ tags: current.filter((t) => t !== tag) });
  }

  return (
    <div className="px-6 py-4 space-y-6 overflow-y-auto flex-1">
      {/* Description */}
      <div>
        <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-2">Description</h3>
        {!canWrite && (
          <p className="mb-2 text-xs text-content-muted">
            Read-only: description editing is disabled.
          </p>
        )}
        {editingDesc ? (
          <div className="space-y-2">
            {/* Toolbar */}
            <div className="flex items-center gap-1">
              <button
                onClick={() => setExpanded((v) => !v)}
                className="flex items-center gap-1 rounded-md px-1.5 py-1 text-[11px] text-content-muted hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
                title={expanded ? 'Collapse editor' : 'Expand editor'}
              >
                {expanded ? <Minimize2 className="h-3.5 w-3.5" /> : <Maximize2 className="h-3.5 w-3.5" />}
                <span>{expanded ? 'Collapse' : 'Expand'}</span>
              </button>
              <button
                onClick={() => setShowPreview((v) => !v)}
                className={cn(
                  'flex items-center gap-1 rounded-md px-1.5 py-1 text-[11px] transition-colors cursor-pointer',
                  showPreview ? 'text-accent bg-accent/10' : 'text-content-muted hover:text-content hover:bg-surface-hover',
                )}
                title={showPreview ? 'Hide preview' : 'Show preview'}
              >
                {showPreview ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
                <span>Preview</span>
              </button>
              <button
                onClick={() => setShowHelp((v) => !v)}
                className={cn(
                  'flex items-center gap-1 rounded-md px-1.5 py-1 text-[11px] transition-colors cursor-pointer',
                  showHelp ? 'text-accent bg-accent/10' : 'text-content-muted hover:text-content hover:bg-surface-hover',
                )}
                title="Markdown help"
              >
                <HelpCircle className="h-3.5 w-3.5" />
                <span>Help</span>
              </button>
            </div>

            {/* Markdown Help Card */}
            {showHelp && (
              <div className="rounded-lg border border-edge bg-surface p-3 text-xs font-mono text-content-secondary leading-relaxed">
                <div className="grid grid-cols-2 gap-x-4 gap-y-0.5">
                  <span># Heading 1</span><span>**bold**</span>
                  <span>## Heading 2</span><span>*italic*</span>
                  <span>### Heading 3</span><span>`inline code`</span>
                  <span>- list item</span><span>1. numbered list</span>
                  <span>[link](url)</span><span>```lang ← syntax highlight</span>
                </div>
                <div className="mt-1.5 pt-1.5 border-t border-edge text-content-muted space-y-0.5">
                  <div>::red[colored text] → <span className="text-red-400">colored</span>, <span className="text-blue-400">blue</span>, <span className="text-green-400">green</span>, <span className="text-yellow-400">yellow</span>, ...</div>
                  <div>SPEC-001 → auto-linked to requirements</div>
                </div>
              </div>
            )}

            {/* Textarea */}
            <textarea
              ref={descRef}
              value={descDraft}
              onChange={(e) => setDescDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); saveDesc(); }
                if (e.key === 'Escape') { e.preventDefault(); cancelDesc(); }
              }}
              rows={expanded ? undefined : 8}
              className={cn(
                'w-full rounded-lg border border-accent bg-surface px-3 py-1.5 text-sm text-content focus:outline-none focus:ring-1 focus:ring-accent resize-y',
                expanded ? 'min-h-[50vh]' : 'min-h-[80px]',
              )}
              placeholder="Add a description (markdown supported)..."
            />

            {/* Preview Pane */}
            {showPreview && (
              <div>
                <span className="text-[10px] font-medium uppercase tracking-wider text-content-muted">Preview</span>
                <div className={cn(
                  'mt-1 rounded-lg border border-edge bg-surface px-3 py-2 overflow-y-auto',
                  expanded ? '' : 'max-h-[40vh]',
                )}>
                  {descDraft.trim() ? (
                    <LinkedMarkdown className="prose prose-sm prose-invert max-w-none text-content prose-headings:text-content prose-strong:text-content prose-code:text-accent prose-code:bg-surface-hover prose-code:px-1.5 prose-code:py-0.5 prose-code:rounded prose-code:text-xs prose-code:before:content-none prose-code:after:content-none prose-pre:bg-surface-hover prose-pre:border prose-pre:border-edge prose-a:text-accent">
                      {descDraft}
                    </LinkedMarkdown>
                  ) : (
                    <span className="text-sm text-content-muted italic">Nothing to preview</span>
                  )}
                </div>
              </div>
            )}

            <div className="flex items-center gap-1.5">
              <button
                onClick={saveDesc}
                className="flex items-center gap-1 rounded-md bg-accent px-2 py-1 text-[11px] font-medium text-white hover:bg-accent-hover transition-colors cursor-pointer"
              >
                <Check className="h-3 w-3" /> Save
              </button>
              <button
                onClick={cancelDesc}
                className="flex items-center gap-1 rounded-md bg-surface-hover px-2 py-1 text-[11px] font-medium text-content-secondary hover:text-content transition-colors cursor-pointer"
              >
                <X className="h-3 w-3" /> Cancel
              </button>
              <span className="text-[10px] text-content-muted ml-auto">Ctrl+Enter to save</span>
            </div>
          </div>
        ) : (
          <div
            onClick={() => {
              if (canWrite) setEditingDesc(true);
            }}
            className={cn(
              'group/desc relative rounded-lg -mx-2 px-2 py-1 transition-colors',
              canWrite ? 'cursor-pointer hover:bg-surface-hover/50' : 'cursor-default',
            )}
            title={canWrite ? 'Click to edit' : undefined}
          >
            {requirement.description ? (
              <LinkedMarkdown className="prose prose-sm prose-invert max-w-none text-content prose-headings:text-content prose-strong:text-content prose-code:text-accent prose-code:bg-surface-hover prose-code:px-1.5 prose-code:py-0.5 prose-code:rounded prose-code:text-xs prose-code:before:content-none prose-code:after:content-none prose-pre:bg-surface-hover prose-pre:border prose-pre:border-edge prose-a:text-accent">
                {requirement.description}
              </LinkedMarkdown>
            ) : (
              <span className="text-sm text-content-muted italic">Add a description...</span>
            )}
            {canWrite && (
              <Pencil className="absolute right-1.5 top-2 h-3 w-3 text-content-muted opacity-0 group-hover/desc:opacity-100 transition-opacity" />
            )}
          </div>
        )}
      </div>

      {/* Metadata */}
      <div>
        <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-3">Details</h3>
        {!canWrite && (
          <p className="mb-3 text-xs text-content-muted">
            Read-only: owner and feature fields cannot be changed.
          </p>
        )}
        <div className="space-y-3">
          {/* Owner — editable */}
          <div className="flex items-center justify-between">
            <span className="text-xs text-content-muted">Owner</span>
            <div className="flex items-center gap-2">
              {requirement.owner && <Avatar name={requirement.owner} size="sm" />}
              <EditableText
                value={requirement.owner}
                onSave={(owner) => save({ owner })}
                className="text-sm text-content"
                placeholder="Assign owner..."
                disabled={!canWrite}
              />
            </div>
          </div>

          {/* Feature — editable */}
          <div className="flex items-center justify-between">
            <span className="text-xs text-content-muted">Feature</span>
            <EditableText
              value={requirement.feature}
              onSave={(feature) => save({ feature })}
              className="text-sm text-content"
              placeholder="Set feature..."
              disabled={!canWrite}
            />
          </div>

          {/* Read-only fields */}
          <div className="flex items-center justify-between">
            <span className="text-xs text-content-muted">Created</span>
            <span className="text-sm text-content">{formatDate(requirement.created_at)}</span>
          </div>
          <div className="flex items-center justify-between">
            <span className="text-xs text-content-muted">Modified</span>
            <span className="text-sm text-content">{formatDate(requirement.modified_at)}</span>
          </div>
        </div>
      </div>

      {/* Tags — editable */}
      <div>
        <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-2">Tags</h3>
        {!canWrite && (
          <p className="mb-2 text-xs text-content-muted">
            Read-only: tag add/remove is disabled.
          </p>
        )}
        <div className="flex flex-wrap gap-1.5 mb-2">
          {(requirement.tags ?? []).map((tag) => (
            <span key={tag} className="inline-flex items-center gap-1 rounded-md bg-surface-hover pl-2 pr-1 py-0.5 text-xs text-content-secondary group/tag">
              {tag}
              <button
                onClick={() => removeTag(tag)}
                disabled={!canWrite}
                title={!canWrite ? 'Read-only: cannot remove tags' : 'Remove tag'}
                className="rounded-sm p-0.5 text-content-muted hover:text-red-400 opacity-0 group-hover/tag:opacity-100 transition-opacity cursor-pointer"
              >
                <X className="h-2.5 w-2.5" />
              </button>
            </span>
          ))}
        </div>
        <div className="flex items-center gap-1.5">
          <input
            type="text"
            value={newTag}
            onChange={(e) => setNewTag(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && addTag()}
            placeholder="Add tag..."
            disabled={!canWrite}
            className="flex-1 rounded-md border border-edge bg-surface px-2 py-1 text-xs text-content placeholder:text-content-muted focus:border-accent focus:outline-none"
          />
          <button
            onClick={addTag}
            disabled={!newTag.trim() || !canWrite}
            title={!canWrite ? 'Read-only: cannot add tags' : 'Add tag'}
            className="flex h-6 w-6 items-center justify-center rounded-md text-content-muted hover:text-accent hover:bg-surface-hover disabled:opacity-30 transition-colors cursor-pointer disabled:cursor-not-allowed"
          >
            <Plus className="h-3.5 w-3.5" />
          </button>
        </div>
      </div>

      {/* AI Evaluation */}
      {requirement.ai_evaluation && (
        <div>
          <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-3">AI Evaluation</h3>
          <div className="space-y-3">
            {/* Score badge */}
            <div className="flex items-center gap-2">
              <span className="text-xs text-content-muted">Quality Score</span>
              <span className={cn(
                'inline-flex items-center justify-center rounded-full px-2 py-0.5 text-xs font-bold min-w-[28px]',
                requirement.ai_evaluation.evaluation.quality_score >= 7
                  ? 'bg-green-500/20 text-green-400'
                  : requirement.ai_evaluation.evaluation.quality_score >= 4
                    ? 'bg-yellow-500/20 text-yellow-400'
                    : 'bg-red-500/20 text-red-400',
              )}>
                {requirement.ai_evaluation.evaluation.quality_score}/10
              </span>
            </div>

            {/* Strengths */}
            {requirement.ai_evaluation.evaluation.strengths.length > 0 && (
              <div>
                <span className="text-xs text-content-muted block mb-1">Strengths</span>
                <ul className="space-y-1">
                  {requirement.ai_evaluation.evaluation.strengths.map((s, i) => (
                    <li key={i} className="text-xs text-content-secondary flex items-start gap-1.5">
                      <span className="text-green-400 mt-0.5 shrink-0">+</span>
                      <span>{s}</span>
                    </li>
                  ))}
                </ul>
              </div>
            )}

            {/* Issues */}
            {requirement.ai_evaluation.evaluation.issues.length > 0 && (
              <div>
                <span className="text-xs text-content-muted block mb-1">Issues</span>
                <div className="space-y-2">
                  {requirement.ai_evaluation.evaluation.issues.map((issue, i) => (
                    <div key={i} className="rounded-md bg-surface-hover px-2.5 py-2 space-y-1">
                      <div className="flex items-center gap-2">
                        <span className={cn(
                          'text-[10px] font-medium uppercase tracking-wide',
                          issue.severity === 'high' ? 'text-red-400'
                            : issue.severity === 'medium' ? 'text-yellow-400'
                              : 'text-content-muted',
                        )}>
                          {issue.severity}
                        </span>
                        <span className="text-[10px] text-content-muted">{issue.type}</span>
                      </div>
                      <p className="text-xs text-content-secondary">{issue.text}</p>
                      {issue.suggestion && (
                        <p className="text-xs text-accent/80 italic">{issue.suggestion}</p>
                      )}
                    </div>
                  ))}
                </div>
              </div>
            )}

            {/* Suggested improvements */}
            {requirement.ai_evaluation.evaluation.suggested_improvements && (
              <div>
                <span className="text-xs text-content-muted block mb-1">Suggested Improvements</span>
                <div className="rounded-md bg-surface-hover px-2.5 py-2 space-y-1">
                  {requirement.ai_evaluation.evaluation.suggested_improvements.description && (
                    <p className="text-xs text-content-secondary">
                      {requirement.ai_evaluation.evaluation.suggested_improvements.description}
                    </p>
                  )}
                  <p className="text-xs text-content-muted italic">
                    {requirement.ai_evaluation.evaluation.suggested_improvements.rationale}
                  </p>
                </div>
              </div>
            )}

            {/* Evaluated timestamp */}
            <div className="text-[10px] text-content-muted pt-1">
              Evaluated {formatDate(requirement.ai_evaluation.evaluated_at)}
            </div>
          </div>
        </div>
      )}

      {/* Dependencies */}
      {requirement.dependencies && requirement.dependencies.length > 0 && (
        <div>
          <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-2">Dependencies</h3>
          <div className="space-y-1">
            {requirement.dependencies.map((dep) => (
              <span key={dep} className="block text-xs font-mono text-accent">{dep}</span>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
