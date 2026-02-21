import { useParams, Link } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { cn } from '../../lib/utils';
import { useDoc } from '../../hooks/useDocs';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';

export function DocFullPage() {
  const { '*': docPath } = useParams();
  const { data: doc, isLoading, error } = useDoc(docPath ?? null);

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <Spinner size="lg" />
      </div>
    );
  }

  if (error || !doc) {
    return (
      <EmptyState
        title="Document not found"
        description={`Could not load "${docPath}".`}
      />
    );
  }

  return (
    <div className="space-y-4">
      {/* Back link + meta */}
      <div className="flex items-center gap-3">
        <Link
          to={`/docs?doc=${encodeURIComponent(doc.path)}`}
          className="inline-flex items-center gap-1.5 text-xs text-content-muted hover:text-content transition-colors"
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          Back to docs
        </Link>
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
        <span className="text-xs text-content-muted">{doc.path}</span>
      </div>

      {/* Rendered markdown */}
      <div className="prose prose-sm prose-invert max-w-none text-content prose-headings:text-content prose-strong:text-content prose-code:text-accent prose-code:bg-surface-hover prose-code:px-1.5 prose-code:py-0.5 prose-code:rounded prose-code:text-xs prose-code:before:content-none prose-code:after:content-none prose-pre:bg-surface-hover prose-pre:border prose-pre:border-edge prose-a:text-accent">
        <Markdown remarkPlugins={[remarkGfm]}>{doc.content}</Markdown>
      </div>
    </div>
  );
}
