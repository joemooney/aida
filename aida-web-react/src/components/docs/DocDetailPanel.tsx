import { useEffect } from 'react';
import { X, ExternalLink } from 'lucide-react';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { cn } from '../../lib/utils';
import { useDoc } from '../../hooks/useDocs';
import { Spinner } from '../ui/Spinner';

interface DocDetailPanelProps {
  path: string;
  onClose: () => void;
}

export function DocDetailPanel({ path, onClose }: DocDetailPanelProps) {
  const { data: doc, isLoading, error } = useDoc(path);

  // Close on Escape
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') onClose();
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [onClose]);

  function openInNewTab() {
    window.open(`/docs/view/${path}`, '_blank');
  }

  return (
    <>
      {/* Backdrop */}
      <div
        className="fixed inset-0 bg-black/40 z-40 animate-fade-in"
        onClick={onClose}
      />

      {/* Panel */}
      <div className="fixed top-0 right-0 bottom-0 z-50 w-full max-w-3xl bg-surface-alt border-l border-edge flex flex-col animate-slide-in-right shadow-2xl shadow-black/40">
        {isLoading ? (
          <div className="flex items-center justify-center flex-1">
            <Spinner size="lg" />
          </div>
        ) : error || !doc ? (
          <div className="flex flex-col items-center justify-center flex-1 text-content-muted">
            <p className="text-sm">Failed to load document</p>
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
                <h2 className="text-base font-semibold text-content truncate">{doc.title}</h2>
                <span
                  className={cn(
                    'shrink-0 inline-flex items-center rounded-md px-2 py-0.5 text-[11px] font-medium',
                    doc.section === 'plans'
                      ? 'bg-amber-500/10 text-amber-400'
                      : 'bg-accent/10 text-accent',
                  )}
                >
                  {doc.section === 'plans' ? 'plan' : 'doc'}
                </span>
              </div>
              <div className="flex items-center gap-1">
                <button
                  onClick={openInNewTab}
                  title="Open in new tab"
                  className="rounded-lg p-1.5 text-content-muted hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
                >
                  <ExternalLink className="h-4 w-4" />
                </button>
                <button
                  onClick={onClose}
                  className="rounded-lg p-1.5 text-content-muted hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
                >
                  <X className="h-4 w-4" />
                </button>
              </div>
            </div>

            {/* Path info */}
            <div className="flex items-center gap-2 px-5 py-2.5 border-b border-edge">
              <span className="text-[11px] font-medium text-content-muted uppercase tracking-wider">Path:</span>
              <span className="text-xs text-content-secondary">{doc.path}</span>
            </div>

            {/* Content */}
            <div className="flex-1 overflow-y-auto">
              <div className="p-5 prose prose-sm prose-invert max-w-none text-content prose-headings:text-content prose-strong:text-content prose-code:text-accent prose-code:bg-surface-hover prose-code:px-1.5 prose-code:py-0.5 prose-code:rounded prose-code:text-xs prose-code:before:content-none prose-code:after:content-none prose-pre:bg-surface-hover prose-pre:border prose-pre:border-edge prose-a:text-accent">
                <Markdown remarkPlugins={[remarkGfm]}>{doc.content}</Markdown>
              </div>
            </div>
          </>
        )}
      </div>
    </>
  );
}
