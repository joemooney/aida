import { cn } from '../../lib/utils';

interface SprintProgressBarProps {
  percentage: number;
  label?: string;
  className?: string;
}

export function SprintProgressBar({ percentage, label, className }: SprintProgressBarProps) {
  return (
    <div className={cn('w-full', className)}>
      <div className="h-1.5 rounded-full bg-surface-hover overflow-hidden">
        <div
          className={cn(
            'h-full rounded-full transition-all duration-300',
            percentage >= 100
              ? 'bg-emerald-500'
              : percentage >= 50
                ? 'bg-accent'
                : 'bg-amber-500',
          )}
          style={{ width: `${Math.min(percentage, 100)}%` }}
        />
      </div>
      {label && (
        <span className="text-[11px] text-content-muted mt-0.5 block">{label}</span>
      )}
    </div>
  );
}
