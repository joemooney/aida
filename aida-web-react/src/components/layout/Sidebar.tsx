import { useMemo } from 'react';
import { NavLink } from 'react-router-dom';
import { LayoutDashboard, Inbox, Columns3, List, Zap, Clock, Sparkles, FileText, Settings, ChevronLeft, ChevronRight } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useRequirements } from '../../hooks/useRequirements';
import {
  getSprintNumber,
  getSprintState,
  getSprintDates,
  computeSprintProgress,
  getSprintAssignmentTarget,
} from '../../lib/sprint-utils';

const navItems = [
  { to: '/', icon: LayoutDashboard, label: 'Dashboard' },
  { to: '/queue', icon: Inbox, label: 'My Queue' },
  { to: '/board', icon: Columns3, label: 'Kanban Board' },
  { to: '/list', icon: List, label: 'List View' },
  { to: '/sprints', icon: Zap, label: 'Sprints' },
  { to: '/timeline', icon: Clock, label: 'Timeline' },
  { to: '/skills', icon: Sparkles, label: 'Skills' },
  { to: '/docs', icon: FileText, label: 'Docs' },
  { to: '/settings', icon: Settings, label: 'Settings' },
];

interface SidebarProps {
  collapsed: boolean;
  onToggle: () => void;
}

export function Sidebar({ collapsed, onToggle }: SidebarProps) {
  const { data: requirements } = useRequirements();

  const activeSprint = useMemo(() => {
    if (!requirements) return null;
    const sprints = requirements.filter((r) => r.req_type === 'Sprint' && !r.archived);
    const active = sprints.find((s) => getSprintState(s) === 'active');
    if (!active) return null;

    const items = requirements.filter((r) => {
      if (r.req_type === 'Sprint' || r.req_type === 'Folder' || r.req_type === 'Meta') return false;
      return getSprintAssignmentTarget(r) === active.id;
    });
    const progress = computeSprintProgress(items);
    const num = getSprintNumber(active);
    const { end } = getSprintDates(active);

    let daysLeft: number | null = null;
    if (end) {
      const diff = Math.ceil((new Date(end).getTime() - Date.now()) / (1000 * 60 * 60 * 24));
      daysLeft = Math.max(0, diff);
    }

    return {
      id: active.spec_id ?? active.id,
      label: num != null ? `Sprint ${num}` : active.title,
      progress,
      daysLeft,
    };
  }, [requirements]);

  return (
    <aside
      className={cn(
        'flex flex-col border-r border-edge bg-surface-alt transition-all duration-200',
        collapsed ? 'w-16' : 'w-56',
      )}
    >
      {/* Logo */}
      <div className={cn('flex items-center gap-3 border-b border-edge px-4 h-14', collapsed && 'justify-center')}>
        <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-accent font-bold text-white text-sm shrink-0">
          A
        </div>
        {!collapsed && <span className="text-base font-semibold text-content tracking-tight">AIDA</span>}
      </div>

      {/* Navigation */}
      <nav className="flex-1 space-y-1 p-2 mt-2">
        {navItems.map(({ to, icon: Icon, label }) => (
          <div key={to}>
            <NavLink
              to={to}
              end={to === '/'}
              className={({ isActive }) =>
                cn(
                  'flex items-center gap-3 rounded-lg px-3 py-2 text-sm font-medium transition-colors',
                  collapsed && 'justify-center px-2',
                  isActive
                    ? 'bg-accent/10 text-accent'
                    : 'text-content-secondary hover:text-content hover:bg-surface-hover',
                )
              }
            >
              <Icon className="h-5 w-5 shrink-0" />
              {!collapsed && label}
            </NavLink>

            {/* Active sprint sub-item under Sprints */}
            {to === '/sprints' && activeSprint && !collapsed && (
              <NavLink
                to="/sprints"
                className="flex items-center gap-2 rounded-lg ml-5 pl-4 pr-3 py-1.5 mt-0.5 text-xs text-content-muted hover:text-content hover:bg-surface-hover transition-colors border-l border-edge"
              >
                <div className="flex-1 min-w-0">
                  <div className="font-medium text-content-secondary truncate">{activeSprint.label}</div>
                  <div className="flex items-center gap-2 mt-1">
                    {/* Progress bar */}
                    <div className="flex-1 h-1 rounded-full bg-surface-hover overflow-hidden">
                      <div
                        className="h-full rounded-full bg-accent transition-all"
                        style={{ width: `${activeSprint.progress.percentage}%` }}
                      />
                    </div>
                    <span className="text-[10px] tabular-nums shrink-0">
                      {activeSprint.progress.completed}/{activeSprint.progress.total}
                    </span>
                  </div>
                  {activeSprint.daysLeft != null && (
                    <div className="text-[10px] mt-0.5">
                      {activeSprint.daysLeft === 0 ? 'Ends today' : `${activeSprint.daysLeft}d left`}
                    </div>
                  )}
                </div>
              </NavLink>
            )}
          </div>
        ))}
      </nav>

      {/* Collapse toggle */}
      <div className="border-t border-edge p-2">
        <button
          onClick={onToggle}
          className="flex w-full items-center justify-center rounded-lg py-2 text-content-muted hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
        >
          {collapsed ? <ChevronRight className="h-4 w-4" /> : <ChevronLeft className="h-4 w-4" />}
        </button>
      </div>
    </aside>
  );
}
