import { cn } from '../../lib/utils';
import type { DocInfo } from '../../api/docs';

interface DocCardProps {
  doc: DocInfo;
  onClick: () => void;
}

export function DocCard({ doc, onClick }: DocCardProps) {
  return (
    <button
      onClick={onClick}
      className={cn(
        'flex flex-col gap-2 rounded-xl border border-edge bg-surface-alt p-4 text-left',
        'hover:border-accent/40 hover:bg-surface-hover transition-colors cursor-pointer',
      )}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="text-sm font-semibold text-content truncate">{doc.title}</span>
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
      <p className="text-xs text-content-muted line-clamp-1 leading-relaxed">
        {doc.path}
      </p>
    </button>
  );
}
