import { QueryBuilder, type RuleGroupType, type Field } from 'react-querybuilder';
import { X } from 'lucide-react';
import { SavedQueryPicker } from './SavedQueryPicker';
import type { SavedQuery } from '../../hooks/useAdvancedQuery';

interface AdvancedQueryBuilderProps {
  query: RuleGroupType;
  onQueryChange: (query: RuleGroupType) => void;
  fields: Field[];
  onClear: () => void;
  hasActiveQuery: boolean;
  savedQueries: SavedQuery[];
  onSaveQuery: (name: string) => void;
  onLoadQuery: (name: string) => void;
  onDeleteQuery: (name: string) => void;
}

const controlClassnames = {
  queryBuilder: 'space-y-2',
  ruleGroup: 'rounded-lg border border-edge bg-surface-alt p-3 space-y-2',
  header: 'flex items-center gap-2 flex-wrap',
  body: 'space-y-2',
  combinators: 'rounded-lg border border-edge bg-surface px-2.5 py-1.5 text-xs text-content focus:border-accent focus:outline-none cursor-pointer',
  addRule: 'text-xs text-accent hover:bg-accent/10 rounded px-2 py-1 transition-colors cursor-pointer',
  addGroup: 'text-xs text-content-muted hover:bg-surface-hover rounded px-2 py-1 transition-colors cursor-pointer',
  removeGroup: 'text-content-muted hover:text-red-400 rounded p-0.5 transition-colors cursor-pointer ml-auto',
  rule: 'flex items-center gap-2 flex-wrap',
  fields: 'rounded-lg border border-edge bg-surface px-2.5 py-1.5 text-xs text-content focus:border-accent focus:outline-none cursor-pointer',
  operators: 'rounded-lg border border-edge bg-surface px-2.5 py-1.5 text-xs text-content focus:border-accent focus:outline-none cursor-pointer',
  value: 'rounded-lg border border-edge bg-surface px-2.5 py-1.5 text-xs text-content focus:border-accent focus:outline-none min-w-[120px]',
  removeRule: 'text-content-muted hover:text-red-400 rounded p-0.5 transition-colors cursor-pointer',
};

export function AdvancedQueryBuilder({
  query,
  onQueryChange,
  fields,
  onClear,
  hasActiveQuery,
  savedQueries,
  onSaveQuery,
  onLoadQuery,
  onDeleteQuery,
}: AdvancedQueryBuilderProps) {
  return (
    <div className="rounded-xl border border-edge bg-surface-alt p-4 space-y-3">
      <div className="flex items-center justify-between">
        <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted">
          Advanced Query
        </h3>
        <div className="flex items-center gap-2">
          <SavedQueryPicker
            savedQueries={savedQueries}
            onSave={onSaveQuery}
            onLoad={onLoadQuery}
            onDelete={onDeleteQuery}
            hasActiveQuery={hasActiveQuery}
          />
          {hasActiveQuery && (
            <button
              onClick={onClear}
              className="flex items-center gap-1 rounded-lg border border-edge bg-surface px-2.5 py-1.5 text-xs text-content-muted hover:text-content hover:bg-surface-hover transition-colors"
            >
              <X className="h-3 w-3" />
              Clear
            </button>
          )}
        </div>
      </div>

      <QueryBuilder
        query={query}
        onQueryChange={onQueryChange}
        fields={fields}
        controlClassnames={controlClassnames}
        combinators={[
          { name: 'and', label: 'AND' },
          { name: 'or', label: 'OR' },
        ]}
        resetOnFieldChange={false}
      />
    </div>
  );
}
