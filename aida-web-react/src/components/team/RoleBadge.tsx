import { cn } from '../../lib/utils';

// trace:STORY-649 | ai:claude

// Color the role badge by seat: advisor (the strategic partner) reads violet,
// implementer (the code-writer) reads blue, anything else falls to a neutral.
function roleConfig(role: string | null): { bg: string; color: string; label: string } {
  const normalized = (role ?? '').trim().toLowerCase();
  switch (normalized) {
    case 'advisor':
    case 'dialog': // deprecated alias for advisor, still accepted upstream
      return { bg: 'bg-violet-500/10', color: 'text-violet-300', label: 'Advisor' };
    case 'implementer':
      return { bg: 'bg-blue-500/10', color: 'text-blue-300', label: 'Implementer' };
    case '':
      return { bg: 'bg-gray-500/10', color: 'text-gray-400', label: 'No role' };
    default:
      return {
        bg: 'bg-slate-500/10',
        color: 'text-slate-300',
        label: role!.charAt(0).toUpperCase() + role!.slice(1),
      };
  }
}

export function RoleBadge({ role }: { role: string | null }) {
  const config = roleConfig(role);
  return (
    <span
      className={cn(
        'inline-flex items-center rounded-md px-2 py-0.5 text-[11px] font-medium',
        config.bg,
        config.color,
      )}
    >
      {config.label}
    </span>
  );
}
