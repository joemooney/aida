// trace:TASK-0001 | ai:claude
import { useState } from 'react';
import {
  ChevronDown,
  ChevronRight,
  Shield,
  ShieldAlert,
  ShieldCheck,
  ShieldQuestion,
  Wrench,
  Bug,
  ClipboardList,
} from 'lucide-react';
import { cn } from '../../lib/utils';
import type { WarningsReport as WarningsReportType, WarningCategory, Warning } from '../../api/skillRunner';

interface WarningsReportProps {
  report: WarningsReportType;
  onAction: (action: string, params: Record<string, unknown>) => void;
  actionPending?: boolean;
}

const RISK_CONFIG: Record<string, { color: string; bgColor: string; borderColor: string; icon: typeof Shield }> = {
  none: { color: 'text-emerald-400', bgColor: 'bg-emerald-500/10', borderColor: 'border-emerald-500/30', icon: ShieldCheck },
  low: { color: 'text-yellow-400', bgColor: 'bg-yellow-500/10', borderColor: 'border-yellow-500/30', icon: Shield },
  medium: { color: 'text-orange-400', bgColor: 'bg-orange-500/10', borderColor: 'border-orange-500/30', icon: ShieldAlert },
  high: { color: 'text-red-400', bgColor: 'bg-red-500/10', borderColor: 'border-red-500/30', icon: ShieldQuestion },
};

function getRiskConfig(riskLevel: string) {
  return RISK_CONFIG[riskLevel] || RISK_CONFIG.medium;
}

function CategoryCard({
  category,
  onAction,
  actionPending,
}: {
  category: WarningCategory;
  onAction: WarningsReportProps['onAction'];
  actionPending?: boolean;
}) {
  const [expanded, setExpanded] = useState(false);
  const config = getRiskConfig(category.riskLevel);
  const Icon = config.icon;

  const actionButton = () => {
    if (category.riskLevel === 'none') {
      return (
        <button
          onClick={(e) => {
            e.stopPropagation();
            onAction('auto_fix', { category: category.name });
          }}
          disabled={actionPending}
          className="inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium bg-emerald-600 text-white hover:bg-emerald-500 transition-colors cursor-pointer disabled:opacity-50"
        >
          <Wrench className="h-3 w-3" />
          Auto-Fix All
        </button>
      );
    }
    if (category.riskLevel === 'low') {
      return (
        <button
          onClick={(e) => {
            e.stopPropagation();
            onAction('create_task', {
              title: `Clean up ${category.name.toLowerCase()} compiler warnings`,
              category: category.name,
              warnings: category.warnings.map((w) => ({
                code: w.code,
                file: w.file,
                line: w.line,
              })),
            });
          }}
          disabled={actionPending}
          className="inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium bg-yellow-600 text-white hover:bg-yellow-500 transition-colors cursor-pointer disabled:opacity-50"
        >
          <ClipboardList className="h-3 w-3" />
          Create Task
        </button>
      );
    }
    return (
      <button
        onClick={(e) => {
          e.stopPropagation();
          onAction('create_defect', {
            title: `Fix ${category.name.toLowerCase()} compiler warnings`,
            category: category.name,
            warnings: category.warnings.map((w) => ({
              code: w.code,
              file: w.file,
              line: w.line,
            })),
          });
        }}
        disabled={actionPending}
        className="inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium bg-orange-600 text-white hover:bg-orange-500 transition-colors cursor-pointer disabled:opacity-50"
      >
        <Bug className="h-3 w-3" />
        Create Defect
      </button>
    );
  };

  return (
    <div className={cn('rounded-lg border', config.borderColor, config.bgColor)}>
      {/* Category header */}
      <button
        onClick={() => setExpanded((e) => !e)}
        className="flex items-center justify-between w-full px-4 py-3 cursor-pointer"
      >
        <div className="flex items-center gap-3">
          {expanded ? (
            <ChevronDown className={cn('h-4 w-4', config.color)} />
          ) : (
            <ChevronRight className={cn('h-4 w-4', config.color)} />
          )}
          <Icon className={cn('h-4 w-4', config.color)} />
          <span className={cn('text-sm font-semibold', config.color)}>
            {category.name}
          </span>
          <span className="text-xs text-content-muted">
            ({category.warnings.length} warning{category.warnings.length !== 1 ? 's' : ''})
          </span>
        </div>
        <div onClick={(e) => e.stopPropagation()}>
          {actionButton()}
        </div>
      </button>

      {/* Description */}
      <div className="px-4 pb-2 text-xs text-content-secondary">
        {category.description}
      </div>

      {/* Expanded warning list */}
      {expanded && (
        <div className="border-t border-edge/30 mx-2 mb-2">
          {category.warnings.map((warning, i) => (
            <WarningRow key={`${warning.file}-${warning.line}-${warning.code}-${i}`} warning={warning} />
          ))}
        </div>
      )}
    </div>
  );
}

function WarningRow({ warning }: { warning: Warning }) {
  return (
    <div className="px-3 py-2 text-xs border-b border-edge/20 last:border-b-0">
      <div className="flex items-start gap-2">
        <span className="font-mono text-accent shrink-0">
          {warning.file}:{warning.line}
        </span>
        <span className="font-mono text-content-muted px-1.5 py-0.5 rounded bg-surface-hover shrink-0">
          {warning.code}
        </span>
      </div>
      {warning.suggestion && (
        <div className="mt-1 text-content-secondary pl-1">
          Suggestion: {warning.suggestion}
        </div>
      )}
    </div>
  );
}

export function WarningsReportView({ report, onAction, actionPending }: WarningsReportProps) {
  const crateEntries = Object.entries(report.crateCounts).sort((a, b) => b[1] - a[1]);

  return (
    <div className="space-y-4">
      {/* Summary bar */}
      <div className="flex items-center gap-4 flex-wrap">
        <div className="text-sm font-semibold text-content">
          {report.totalWarnings} warning{report.totalWarnings !== 1 ? 's' : ''}
        </div>
        <div className="flex items-center gap-2 flex-wrap">
          {crateEntries.map(([name, count]) => (
            <span
              key={name}
              className="inline-flex items-center rounded-md bg-surface-hover px-2 py-0.5 text-[11px] font-medium text-content-secondary"
            >
              {name}: {count}
            </span>
          ))}
        </div>
      </div>

      {/* Category cards */}
      {report.categories.map((category) => (
        <CategoryCard
          key={category.name}
          category={category}
          onAction={onAction}
          actionPending={actionPending}
        />
      ))}

      {report.totalWarnings === 0 && (
        <div className="text-center py-8 text-content-muted text-sm">
          No warnings found — clean build!
        </div>
      )}
    </div>
  );
}
