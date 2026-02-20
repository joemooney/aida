import type { HTMLAttributes } from 'react';
import { cn } from '../../lib/utils';

interface CardProps extends HTMLAttributes<HTMLDivElement> {
  hover?: boolean;
}

export function Card({ hover = false, className, ...props }: CardProps) {
  return (
    <div
      className={cn(
        'rounded-xl border border-edge bg-surface-alt p-4',
        hover && 'transition-all hover:border-edge-hover hover:shadow-lg hover:shadow-black/10 hover:-translate-y-0.5',
        className,
      )}
      {...props}
    />
  );
}
