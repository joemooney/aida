import { cn } from '../../lib/utils';
import type { SkillInfo } from '../../api/skills';

interface SkillCardProps {
  skill: SkillInfo;
  onClick: () => void;
}

export function SkillCard({ skill, onClick }: SkillCardProps) {
  return (
    <button
      onClick={onClick}
      className={cn(
        'flex flex-col gap-2 rounded-xl border border-edge bg-surface-alt p-4 text-left',
        'hover:border-accent/40 hover:bg-surface-hover transition-colors cursor-pointer',
      )}
    >
      <div className="flex items-center justify-between gap-2">
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
      </div>
      <p className="text-xs text-content-muted line-clamp-2 leading-relaxed">
        {skill.description || 'No description'}
      </p>
    </button>
  );
}
