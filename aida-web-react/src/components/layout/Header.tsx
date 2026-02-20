import { useState, useRef, useEffect } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { Search, Sun, Moon, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useTheme } from '../../hooks/useTheme';
import { useSearch } from '../../hooks/useSearch';
import type { Requirement } from '@shared/types';

export function Header() {
  const { theme, toggle } = useTheme();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const [query, setQuery] = useState(searchParams.get('q') ?? '');
  const [showResults, setShowResults] = useState(false);
  const { data: results, isLoading } = useSearch(query);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === '/' && !e.ctrlKey && !e.metaKey) {
        const target = e.target as HTMLElement;
        if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA') return;
        e.preventDefault();
        inputRef.current?.focus();
      }
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, []);

  function handleSelect(req: Requirement) {
    setShowResults(false);
    setQuery('');
    navigate(`?detail=${req.spec_id ?? req.id}`);
  }

  return (
    <header className="flex items-center gap-4 border-b border-edge bg-surface-alt px-6 h-14">
      {/* Search */}
      <div className="relative flex-1 max-w-lg">
        <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-content-muted" />
        <input
          ref={inputRef}
          type="text"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setShowResults(true);
          }}
          onFocus={() => query.length > 0 && setShowResults(true)}
          onBlur={() => setTimeout(() => setShowResults(false), 200)}
          placeholder="Search requirements... ( / )"
          className="w-full rounded-lg border border-edge bg-surface py-1.5 pl-9 pr-8 text-sm text-content placeholder:text-content-muted focus:border-accent focus:outline-none focus:ring-1 focus:ring-accent"
        />
        {query && (
          <button
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => { setQuery(''); setShowResults(false); }}
            className="absolute right-2.5 top-1/2 -translate-y-1/2 text-content-muted hover:text-content cursor-pointer"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        )}

        {/* Search results dropdown */}
        {showResults && query.length > 0 && (
          <div className="absolute top-full left-0 right-0 mt-1 rounded-lg border border-edge bg-surface-alt shadow-xl shadow-black/20 z-50 max-h-80 overflow-y-auto animate-fade-in">
            {isLoading ? (
              <div className="px-4 py-3 text-sm text-content-muted">Searching...</div>
            ) : results && results.length > 0 ? (
              results.slice(0, 10).map((req) => (
                <button
                  key={req.id}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => handleSelect(req)}
                  className="flex w-full items-center gap-3 px-4 py-2.5 text-left hover:bg-surface-hover transition-colors cursor-pointer"
                >
                  <span className="text-[11px] font-mono text-content-muted shrink-0">{req.spec_id}</span>
                  <span className="text-sm text-content truncate">{req.title}</span>
                </button>
              ))
            ) : (
              <div className="px-4 py-3 text-sm text-content-muted">No results found</div>
            )}
          </div>
        )}
      </div>

      {/* Spacer */}
      <div className="flex-1" />

      {/* Theme toggle */}
      <button
        onClick={toggle}
        className={cn(
          'flex h-8 w-8 items-center justify-center rounded-lg transition-colors cursor-pointer',
          'text-content-muted hover:text-content hover:bg-surface-hover',
        )}
        title={`Switch to ${theme === 'dark' ? 'light' : 'dark'} mode`}
      >
        {theme === 'dark' ? <Sun className="h-4 w-4" /> : <Moon className="h-4 w-4" />}
      </button>
    </header>
  );
}
