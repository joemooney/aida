import { useState, useRef, useEffect, useCallback } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { Search, Sun, Moon, X, RefreshCw, Plus, LogOut } from 'lucide-react';
import { useQueryClient } from '@tanstack/react-query';
import { cn } from '../../lib/utils';
import { useTheme } from '../../hooks/useTheme';
import { useSearch } from '../../hooks/useSearch';
import { useFilters, type Filters } from '../../hooks/useFilters';
import { reloadServer } from '../../api/requirements';
import { QuickCreateDropdown } from '../create/QuickCreateDropdown';
import { CreateRequirementModal } from '../create/CreateRequirementModal';
import type { Requirement } from '@shared/types';
import { useAuth } from '../../hooks/useAuth';
import { usePermissions } from '../../hooks/usePermissions';

const FILTER_FIELDS = new Set<keyof Filters>(['status', 'priority', 'type', 'feature', 'owner', 'tag']);

function normalizeFilterValue(key: keyof Filters, value: string): string {
  if (key === 'status' || key === 'priority') {
    // Title-case: "approved" -> "Approved", "in-progress" -> "In-Progress"
    return value.replace(/\b\w/g, (c) => c.toUpperCase());
  }
  if (key === 'type') {
    // Title-case type names: "bug" -> "Bug", "functional" -> "Functional"
    return value.replace(/\b\w/g, (c) => c.toUpperCase());
  }
  return value;
}

function parseStructuredQuery(input: string): { filters: Partial<Filters>; remainder: string } {
  const filters: Partial<Filters> = {};
  // Match field:value or field:"quoted value"
  const pattern = /\b(status|priority|type|feature|owner|tag):(?:"([^"]+)"|(\S+))/gi;
  let remainder = input;

  let match;
  while ((match = pattern.exec(input)) !== null) {
    const key = match[1].toLowerCase() as keyof Filters;
    const value = match[2] ?? match[3]; // quoted or unquoted
    if (FILTER_FIELDS.has(key)) {
      filters[key] = normalizeFilterValue(key, value) as never;
      remainder = remainder.replace(match[0], '');
    }
  }

  return { filters, remainder: remainder.trim() };
}

export function Header() {
  const { theme, toggle } = useTheme();
  const { authEnabled, user, logout } = useAuth();
  const { canWrite, role } = usePermissions();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [searchParams] = useSearchParams();
  const [query, setQuery] = useState(searchParams.get('q') ?? '');
  const [showResults, setShowResults] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [dropdownOpen, setDropdownOpen] = useState(false);
  const [modalOpen, setModalOpen] = useState(false);
  const [modalPreFill, setModalPreFill] = useState<{ type?: string; title?: string }>({});
  const { data: results, isLoading } = useSearch(query);
  const { setFilter } = useFilters();
  const inputRef = useRef<HTMLInputElement>(null);
  const createBtnRef = useRef<HTMLDivElement>(null);

  const openDropdown = useCallback(() => setDropdownOpen(true), []);

  // Listen for custom event from hotkey
  useEffect(() => {
    document.addEventListener('aida:quick-create', openDropdown);
    return () => document.removeEventListener('aida:quick-create', openDropdown);
  }, [openDropdown]);

  async function handleRefresh() {
    setRefreshing(true);
    try {
      await reloadServer();
      await queryClient.invalidateQueries();
    } catch {
      // Still invalidate local cache even if server reload fails
      await queryClient.invalidateQueries();
    } finally {
      setRefreshing(false);
    }
  }

  function handleSelect(req: Requirement) {
    setShowResults(false);
    setQuery('');
    navigate(`?detail=${req.spec_id ?? req.id}`);
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === 'Enter' && query.trim()) {
      const { filters, remainder } = parseStructuredQuery(query);
      const filterKeys = Object.keys(filters) as (keyof Filters)[];
      if (filterKeys.length > 0) {
        for (const key of filterKeys) {
          setFilter(key, filters[key]!);
        }
        setQuery(remainder);
        if (!remainder) setShowResults(false);
      }
    }
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
          onKeyDown={handleKeyDown}
          placeholder="Search... (try owner:joe, tag:frontend)"
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

      {/* Create requirement */}
      {canWrite && (
        <>
          <div ref={createBtnRef} className="relative">
            <button
              onClick={() => setDropdownOpen((v) => !v)}
              className="flex items-center gap-1.5 rounded-lg bg-accent px-3 py-1.5 text-sm font-medium text-white hover:bg-accent/90 transition-colors cursor-pointer"
            >
              <Plus className="h-4 w-4" />
              New
            </button>
            {dropdownOpen && (
              <QuickCreateDropdown
                onClose={() => setDropdownOpen(false)}
                onMoreOptions={(type, title) => {
                  setDropdownOpen(false);
                  setModalPreFill({ type, title });
                  setModalOpen(true);
                }}
              />
            )}
          </div>
          {modalOpen && (
            <CreateRequirementModal
              onClose={() => { setModalOpen(false); setModalPreFill({}); }}
              initialType={modalPreFill.type}
              initialTitle={modalPreFill.title}
            />
          )}
        </>
      )}

      {/* Refresh data */}
      <button
        onClick={handleRefresh}
        disabled={refreshing}
        className={cn(
          'flex h-8 w-8 items-center justify-center rounded-lg transition-colors cursor-pointer',
          'text-content-muted hover:text-content hover:bg-surface-hover',
          refreshing && 'animate-spin',
        )}
        title="Refresh data from server"
      >
        <RefreshCw className="h-4 w-4" />
      </button>

      {/* Theme toggle */}
      {authEnabled && user && (
        <>
          <span
            className={cn(
              'rounded-full px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide',
              role === 'admin'
                ? 'bg-red-500/15 text-red-300'
                : role === 'editor'
                  ? 'bg-blue-500/15 text-blue-300'
                  : 'bg-amber-500/15 text-amber-300',
            )}
            title={canWrite ? 'You can edit data' : 'Read-only access'}
          >
            {role}
          </span>
          <div className="text-xs text-content-secondary">
            <span className="font-medium text-content">{user.name}</span>
            <span className="ml-1 text-content-muted">@{user.handle}</span>
          </div>
          <button
            onClick={() => void logout()}
            className={cn(
              'flex h-8 w-8 items-center justify-center rounded-lg transition-colors cursor-pointer',
              'text-content-muted hover:text-content hover:bg-surface-hover',
            )}
            title="Sign out"
          >
            <LogOut className="h-4 w-4" />
          </button>
        </>
      )}

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
