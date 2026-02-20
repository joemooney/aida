import { NavLink } from 'react-router-dom';
import { LayoutDashboard, Columns3, List, Zap, Sparkles, ChevronLeft, ChevronRight } from 'lucide-react';
import { cn } from '../../lib/utils';

const navItems = [
  { to: '/', icon: LayoutDashboard, label: 'Dashboard' },
  { to: '/board', icon: Columns3, label: 'Kanban Board' },
  { to: '/list', icon: List, label: 'List View' },
  { to: '/sprints', icon: Zap, label: 'Sprints' },
  { to: '/skills', icon: Sparkles, label: 'Skills' },
];

interface SidebarProps {
  collapsed: boolean;
  onToggle: () => void;
}

export function Sidebar({ collapsed, onToggle }: SidebarProps) {
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
          <NavLink
            key={to}
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
