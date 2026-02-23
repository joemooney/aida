import { useState, useMemo } from 'react';
import { useSearchParams } from 'react-router-dom';
import { Sparkles, Search, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useSkills } from '../../hooks/useSkills';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { SkillCard } from './SkillCard';
import { SkillDetailPanel } from './SkillDetailPanel';
import { SkillRunnerPanel } from './SkillRunnerPanel';

type FilterKind = 'all' | 'skill' | 'command';

const filters: { value: FilterKind; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'skill', label: 'Skills' },
  { value: 'command', label: 'Commands' },
];

export function SkillsView() {
  const { data: skills, isLoading, error } = useSkills();
  const [filter, setFilter] = useState<FilterKind>('all');
  const [search, setSearch] = useState('');
  const [searchParams, setSearchParams] = useSearchParams();
  const selectedSkill = searchParams.get('skill');
  const [runningSkill, setRunningSkill] = useState<{ name: string; description: string } | null>(null);

  const filtered = useMemo(() => {
    if (!skills) return [];
    let result = skills;
    if (filter !== 'all') {
      result = result.filter((s) => s.kind === filter);
    }
    if (search.trim()) {
      const q = search.toLowerCase();
      result = result.filter(
        (s) =>
          s.name.toLowerCase().includes(q) ||
          s.description.toLowerCase().includes(q) ||
          s.content.toLowerCase().includes(q),
      );
    }
    return result;
  }, [skills, filter, search]);

  function openSkill(name: string) {
    setSearchParams({ skill: name });
  }

  function closeSkill() {
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
        title="Failed to load skills"
        description="Make sure the AIDA server is running on port 8080."
      />
    );
  }

  return (
    <div className="space-y-4">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Sparkles className="h-5 w-5 text-accent" />
          <h1 className="text-xl font-semibold text-content">Skills & Commands</h1>
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
            placeholder="Search skills..."
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

      {/* Grid */}
      {filtered.length === 0 ? (
        <EmptyState
          icon={<Sparkles className="h-10 w-10" />}
          title="No skills found"
          description="No skills or commands match the current filter."
        />
      ) : (
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-3">
          {filtered.map((skill) => (
            <SkillCard
              key={`${skill.kind}-${skill.name}`}
              skill={skill}
              onClick={() => openSkill(skill.name)}
              onRun={() => setRunningSkill({ name: skill.name, description: skill.description })}
            />
          ))}
        </div>
      )}

      {/* Detail panel */}
      {selectedSkill && (
        <SkillDetailPanel name={selectedSkill} onClose={closeSkill} />
      )}

      {/* Skill runner panel */}
      {runningSkill && (
        <SkillRunnerPanel
          skillName={runningSkill.name}
          skillDescription={runningSkill.description}
          onClose={() => setRunningSkill(null)}
        />
      )}
    </div>
  );
}
