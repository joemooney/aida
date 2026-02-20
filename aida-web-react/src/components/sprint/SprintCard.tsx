import { Calendar } from 'lucide-react';
import type { Requirement } from '@shared/types';
import { cn, formatDate } from '../../lib/utils';
import {
  getSprintNumber,
  getSprintGoal,
  getSprintDates,
  getSprintState,
  computeSprintProgress,
  type SprintState,
} from '../../lib/sprint-utils';
import { SprintProgressBar } from './SprintProgressBar';

const stateBadge: Record<SprintState, { label: string; color: string; bg: string }> = {
  active:  { label: 'Active',   color: 'text-emerald-400', bg: 'bg-emerald-500/10' },
  future:  { label: 'Upcoming', color: 'text-blue-400',    bg: 'bg-blue-500/10' },
  past:    { label: 'Past',     color: 'text-gray-400',    bg: 'bg-gray-500/10' },
  unknown: { label: 'No dates', color: 'text-gray-400',    bg: 'bg-gray-500/10' },
};

interface SprintCardProps {
  sprint: Requirement;
  items: Requirement[];
  selected: boolean;
  onClick: () => void;
}

export function SprintCard({ sprint, items, selected, onClick }: SprintCardProps) {
  const num = getSprintNumber(sprint);
  const goal = getSprintGoal(sprint);
  const { start, end } = getSprintDates(sprint);
  const state = getSprintState(sprint);
  const progress = computeSprintProgress(items);
  const badge = stateBadge[state];

  return (
    <button
      onClick={onClick}
      className={cn(
        'flex flex-col gap-2 rounded-xl border bg-surface-alt p-4 min-w-[240px] max-w-[280px] shrink-0 text-left transition-all cursor-pointer',
        selected
          ? 'border-accent bg-accent/5 shadow-md shadow-accent/10'
          : 'border-edge hover:border-edge-hover hover:bg-surface-hover',
      )}
    >
      {/* Header */}
      <div className="flex items-center justify-between gap-2">
        <span className="text-sm font-semibold text-content truncate">
          {num != null ? `Sprint ${num}` : sprint.title}
        </span>
        <span className={cn('inline-flex items-center rounded-full px-2 py-0.5 text-[11px] font-medium shrink-0', badge.bg, badge.color)}>
          {badge.label}
        </span>
      </div>

      {/* Goal */}
      {goal && (
        <p className="text-xs text-content-secondary line-clamp-1">{goal}</p>
      )}

      {/* Dates */}
      {start && end && (
        <div className="flex items-center gap-1 text-[11px] text-content-muted">
          <Calendar className="h-3 w-3" />
          <span>{formatDate(start)} - {formatDate(end)}</span>
        </div>
      )}

      {/* Progress */}
      <SprintProgressBar
        percentage={progress.percentage}
        label={`${progress.completed}/${progress.total} items${progress.totalPoints > 0 ? ` · ${progress.completedPoints}/${progress.totalPoints} pts` : ''}`}
      />
    </button>
  );
}
