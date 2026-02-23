import { Play } from 'lucide-react';
import { cn } from '../../lib/utils';
import type { SkillInfo } from '../../api/skills';

// Skills that can be run from the web UI
const RUNNABLE_SKILLS = new Set(['aida-compiler-warnings']);

interface SkillCardProps {
  skill: SkillInfo;
  onClick: () => void;
  onRun?: () => void;
}

export function SkillCard({ skill, onClick, onRun }: SkillCardProps) {
  const isRunnable = RUNNABLE_SKILLS.has(skill.name);

  return (
    <div
      className={cn(
        'flex flex-col gap-2 rounded-xl border border-edge bg-surface-alt p-4 text-left',
        'hover:border-accent/40 hover:bg-surface-hover transition-colors',
      )}
    >
      <button onClick={onClick} className="flex items-center justify-between gap-2 cursor-pointer">
        <span className="text-sm font-semibold text-content truncate">{skill.name}</span>
        <span
          className={cn(
            'shrink-0 inline-flex items-center rounded-md px-2 py-0.5 text-[11px] font-medium',
            skill.kind === 'skill'
              ? 'bg-accent/10 text-accent'
              : 'bg-amber-500/10 text-amber-400',
          )}
        >
          {skill.kind}
        </span>
      </button>
      <button onClick={onClick} className="cursor-pointer text-left">
        <p className="text-xs text-content-muted line-clamp-2 leading-relaxed">
          {skill.description || 'No description'}
        </p>
      </button>
      {isRunnable && onRun && (
        <button
          onClick={(e) => {
            e.stopPropagation();
            onRun();
          }}
          className="inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium bg-accent text-white hover:bg-accent-hover transition-colors cursor-pointer mt-1 w-fit"
        >
          <Play className="h-3 w-3" />
          Run
        </button>
      )}
    </div>
  );
}
