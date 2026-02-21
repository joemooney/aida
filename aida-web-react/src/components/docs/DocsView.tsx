import { useState, useMemo } from 'react';
import { useSearchParams } from 'react-router-dom';
import { FileText, Search, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useDocs } from '../../hooks/useDocs';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { DocCard } from './DocCard';
import { DocDetailPanel } from './DocDetailPanel';

type FilterSection = 'all' | 'docs' | 'plans';

const filters: { value: FilterSection; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'docs', label: 'Docs' },
  { value: 'plans', label: 'Plans' },
];

export function DocsView() {
  const { data: docs, isLoading, error } = useDocs();
  const [filter, setFilter] = useState<FilterSection>('all');
  const [search, setSearch] = useState('');
  const [searchParams, setSearchParams] = useSearchParams();
  const selectedDoc = searchParams.get('doc');

  const filtered = useMemo(() => {
    if (!docs) return [];
    let result = docs;
    if (filter !== 'all') {
      result = result.filter((d) => d.section === filter);
    }
    if (search.trim()) {
      const q = search.toLowerCase();
      result = result.filter(
        (d) =>
          d.name.toLowerCase().includes(q) ||
          d.title.toLowerCase().includes(q) ||
          d.path.toLowerCase().includes(q),
      );
    }
    return result;
  }, [docs, filter, search]);

  const docsList = filtered.filter((d) => d.section === 'docs');
  const plansList = filtered.filter((d) => d.section === 'plans');

  function openDoc(path: string) {
    setSearchParams({ doc: path });
  }

  function closeDoc() {
    setSearchParams({});
  }

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <Spinner size="lg" />
      </div>
    );
  }

  if (error) {
    return (
      <EmptyState
        title="Failed to load docs"
        description="Make sure the AIDA server is running on port 8080."
      />
    );
  }

  return (
    <div className="space-y-4">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <FileText className="h-5 w-5 text-accent" />
          <h1 className="text-xl font-semibold text-content">Documentation</h1>
          <span className="text-sm text-content-muted">({filtered.length})</span>
        </div>
      </div>

      {/* Filter toggles + search */}
      <div className="flex items-center gap-3 flex-wrap">
        <div className="flex gap-1 rounded-lg bg-surface-alt border border-edge p-1 w-fit">
          {filters.map((f) => (
            <button
              key={f.value}
              onClick={() => setFilter(f.value)}
              className={cn(
                'rounded-md px-3 py-1.5 text-xs font-medium transition-colors cursor-pointer',
                filter === f.value
                  ? 'bg-accent text-white'
                  : 'text-content-secondary hover:text-content hover:bg-surface-hover',
              )}
            >
              {f.label}
            </button>
          ))}
        </div>
        <div className="relative">
          <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-content-muted" />
          <input
            type="text"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search docs..."
            className="h-8 rounded-lg border border-edge bg-surface-alt pl-8 pr-8 text-xs text-content placeholder:text-content-muted focus:outline-none focus:border-accent/50 w-56"
          />
          {search && (
            <button
              onClick={() => setSearch('')}
              className="absolute right-2 top-1/2 -translate-y-1/2 text-content-muted hover:text-content cursor-pointer"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          )}
        </div>
      </div>

      {/* Content */}
      {filtered.length === 0 ? (
        <EmptyState
          icon={<FileText className="h-10 w-10" />}
          title="No documents found"
          description={docs?.length ? 'No documents match the current filter.' : 'No markdown files found in the docs/ directory.'}
        />
      ) : (
        <div className="space-y-6">
          {/* Documentation section */}
          {docsList.length > 0 && (
            <section>
              <h2 className="text-sm font-medium text-content-muted uppercase tracking-wider mb-3">Documentation</h2>
              <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-3">
                {docsList.map((doc) => (
                  <DocCard key={doc.path} doc={doc} onClick={() => openDoc(doc.path)} />
                ))}
              </div>
            </section>
          )}

          {/* Plans section */}
          {plansList.length > 0 && (
            <section>
              <h2 className="text-sm font-medium text-content-muted uppercase tracking-wider mb-3">Plans</h2>
              <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-3">
                {plansList.map((doc) => (
                  <DocCard key={doc.path} doc={doc} onClick={() => openDoc(doc.path)} />
                ))}
              </div>
            </section>
          )}
        </div>
      )}

      {/* Detail panel */}
      {selectedDoc && (
        <DocDetailPanel path={selectedDoc} onClose={closeDoc} />
      )}
    </div>
  );
}
