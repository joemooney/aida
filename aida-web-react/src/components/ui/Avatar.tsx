import { cn, getInitials } from '../../lib/utils';
import { AVATAR_COLORS } from '../../lib/constants';

function hashCode(str: string): number {
  let hash = 0;
  for (let i = 0; i < str.length; i++) {
    hash = str.charCodeAt(i) + ((hash << 5) - hash);
  }
  return Math.abs(hash);
}

interface AvatarProps {
  name: string;
  size?: 'sm' | 'md' | 'lg';
  className?: string;
}

export function Avatar({ name, size = 'md', className }: AvatarProps) {
  const colorIndex = hashCode(name) % AVATAR_COLORS.length;
  const bg = AVATAR_COLORS[colorIndex];
  const initials = getInitials(name);

  const sizeClasses = {
    sm: 'h-6 w-6 text-[10px]',
    md: 'h-8 w-8 text-xs',
    lg: 'h-10 w-10 text-sm',
  };

  return (
    <div
      className={cn(
        'inline-flex items-center justify-center rounded-full font-semibold text-white shrink-0',
        bg,
        sizeClasses[size],
        className,
      )}
      title={name}
    >
      {initials}
    </div>
  );
}
