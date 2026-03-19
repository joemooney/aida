import { useQuery } from '@tanstack/react-query';
import { ArrowLeftRight, CheckCircle2, AlertTriangle, XCircle, HelpCircle, RefreshCw, ExternalLink } from 'lucide-react';
import { fetchJiraSync } from '../../api/jira';
import type { JiraSyncItem } from '../../api/jira';

export function JiraSyncView() {
  const { data, isLoading, error, refetch, isFetching } = useQuery({
    queryKey: ['jira-sync'],
    queryFn: fetchJiraSync,
    staleTime: 60_000,
  });

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-accent" />
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center h-full text-content-muted">
        <XCircle className="h-12 w-12 mb-4 text-red-400" />
        <p className="text-lg font-medium">Failed to load Jira sync status</p>
        <p className="text-sm mt-1">{(error as Error).message}</p>
        <button
          onClick={() => refetch()}
          className="mt-4 px-4 py-2 bg-accent text-white rounded-lg hover:bg-accent/80"
        >
          Retry
        </button>
      </div>
    );
  }

  if (!data || data.items.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center h-full text-content-muted">
        <ArrowLeftRight className="h-12 w-12 mb-4" />
        <p className="text-lg font-medium">No linked Jira issues</p>
        <p className="text-sm mt-2">Push requirements to Jira to start syncing:</p>
        <code className="mt-2 px-3 py-1 bg-surface rounded text-sm font-mono">
          aida jira push FR-001
        </code>
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="shrink-0 border-b border-edge px-6 py-4">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <ArrowLeftRight className="h-5 w-5 text-accent" />
            <h1 className="text-lg font-semibold text-content">Jira Sync</h1>
          </div>
          <button
            onClick={() => refetch()}
            disabled={isFetching}
            className="flex items-center gap-2 px-3 py-1.5 text-sm bg-surface hover:bg-surface-hover rounded-lg transition-colors disabled:opacity-50"
          >
            <RefreshCw className={`h-4 w-4 ${isFetching ? 'animate-spin' : ''}`} />
            Refresh
          </button>
        </div>

        {/* Summary stats */}
        <div className="flex gap-4 mt-3">
          <StatBadge
            icon={<CheckCircle2 className="h-4 w-4" />}
            label="In Sync"
            count={data.in_sync}
            color="text-green-400"
          />
          <StatBadge
            icon={<AlertTriangle className="h-4 w-4" />}
            label="Drifted"
            count={data.drifted}
            color="text-amber-400"
          />
          <StatBadge
            icon={<XCircle className="h-4 w-4" />}
            label="Errors"
            count={data.errors}
            color="text-red-400"
          />
          <span className="text-sm text-content-muted ml-auto">
            {data.total} linked items
          </span>
        </div>
      </div>

      {/* Items list */}
      <div className="flex-1 overflow-y-auto px-6 py-4">
        <div className="space-y-2">
          {data.items.map((item) => (
            <SyncItem key={`${item.aida_id}-${item.jira_key}`} item={item} />
          ))}
        </div>
      </div>
    </div>
  );
}

function StatBadge({ icon, label, count, color }: {
  icon: React.ReactNode;
  label: string;
  count: number;
  color: string;
}) {
  return (
    <div className={`flex items-center gap-1.5 text-sm ${color}`}>
      {icon}
      <span className="font-medium">{count}</span>
      <span className="text-content-muted">{label}</span>
    </div>
  );
}

function SyncItem({ item }: { item: JiraSyncItem }) {
  const statusConfig = {
    in_sync: { icon: CheckCircle2, color: 'text-green-400', bg: 'bg-green-400/10', label: 'In Sync' },
    drifted: { icon: AlertTriangle, color: 'text-amber-400', bg: 'bg-amber-400/10', label: 'Drifted' },
    error: { icon: XCircle, color: 'text-red-400', bg: 'bg-red-400/10', label: 'Error' },
    unchecked: { icon: HelpCircle, color: 'text-content-muted', bg: 'bg-surface', label: 'Unchecked' },
  };

  const config = statusConfig[item.sync_status] || statusConfig.unchecked;
  const StatusIcon = config.icon;

  return (
    <div className={`rounded-lg border border-edge ${config.bg} p-4`}>
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <StatusIcon className={`h-5 w-5 ${config.color}`} />
          <div>
            <div className="flex items-center gap-2">
              <span className="text-sm font-mono text-content-muted">{item.aida_id}</span>
              <span className="text-content-muted">↔</span>
              <a
                href={`https://joemooney.atlassian.net/browse/${item.jira_key}`}
                target="_blank"
                rel="noopener noreferrer"
                className="text-sm font-mono text-accent hover:underline flex items-center gap-1"
              >
                {item.jira_key}
                <ExternalLink className="h-3 w-3" />
              </a>
            </div>
            <p className="text-sm text-content mt-0.5">{item.aida_title}</p>
          </div>
        </div>
        <span className={`text-xs px-2 py-0.5 rounded ${config.color} ${config.bg} font-medium`}>
          {config.label}
        </span>
      </div>

      {/* Show diffs if drifted */}
      {item.diffs.length > 0 && (
        <div className="mt-3 ml-8 space-y-1">
          {item.diffs.map((diff, i) => (
            <div key={i} className="text-xs">
              <span className="text-content-muted font-medium">{diff.field}:</span>
              <span className="text-red-400 ml-2">AIDA: {diff.aida_value}</span>
              <span className="text-content-muted mx-1">→</span>
              <span className="text-amber-400">Jira: {diff.jira_value}</span>
            </div>
          ))}
        </div>
      )}

      {/* Status comparison */}
      {item.sync_status === 'in_sync' && (
        <div className="mt-2 ml-8 text-xs text-content-muted">
          AIDA: {item.aida_status} | Jira: {item.jira_status}
        </div>
      )}
    </div>
  );
}
