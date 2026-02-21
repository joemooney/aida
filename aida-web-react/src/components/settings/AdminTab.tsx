// trace:TASK-0373 | ai:claude
import { useEffect, useRef, useState } from 'react';
import { Card } from '../ui/Card';
import { Spinner } from '../ui/Spinner';
import { useAdminStatus, useRebuild, useApiKeys, useSetApiKey, useDeleteApiKey } from '../../hooks/useAdmin';
import type { BuildPhase } from '../../hooks/useAdmin';

function formatUptime(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  if (h > 0) return `${h}h ${m}m ${s}s`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

function phaseLabel(phase: BuildPhase): string {
  switch (phase) {
    case 'idle': return 'Ready';
    case 'building': return 'Building...';
    case 'success': return 'Build succeeded';
    case 'failed': return 'Build failed';
    case 'restarting': return 'Restarting server...';
    case 'reconnecting': return 'Waiting for server...';
  }
}

function ApiKeysCard() {
  const { data: apiKeys, isLoading } = useApiKeys();
  const setApiKeyMutation = useSetApiKey();
  const deleteApiKeyMutation = useDeleteApiKey();
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const [keyValue, setKeyValue] = useState('');

  const handleSave = (name: string) => {
    if (!keyValue.trim()) return;
    setApiKeyMutation.mutate(
      { name, value: keyValue.trim() },
      {
        onSuccess: () => {
          setEditingKey(null);
          setKeyValue('');
        },
      },
    );
  };

  const handleClear = (name: string) => {
    deleteApiKeyMutation.mutate(name);
  };

  const handleCancel = () => {
    setEditingKey(null);
    setKeyValue('');
  };

  if (isLoading) {
    return (
      <Card>
        <h2 className="text-lg font-semibold text-content mb-4">API Keys</h2>
        <div className="flex items-center gap-2 text-content-secondary text-sm">
          <Spinner size="sm" /> Loading...
        </div>
      </Card>
    );
  }

  return (
    <Card>
      <h2 className="text-lg font-semibold text-content mb-4">API Keys</h2>
      <div className="space-y-4">
        {apiKeys?.map((key) => (
          <div key={key.name} className="flex flex-col gap-2">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-3">
                <span className="text-sm font-mono text-content">{key.name}</span>
                {key.isSet && (
                  <span
                    className={`text-xs px-1.5 py-0.5 rounded ${
                      key.source === 'env'
                        ? 'bg-blue-500/20 text-blue-400'
                        : 'bg-green-500/20 text-green-400'
                    }`}
                  >
                    {key.source}
                  </span>
                )}
              </div>
              <div className="flex items-center gap-2">
                {editingKey !== key.name && (
                  <>
                    <button
                      onClick={() => setEditingKey(key.name)}
                      className="px-3 py-1 text-xs bg-surface border border-edge text-content rounded hover:bg-surface-alt transition-colors"
                    >
                      {key.isSet ? 'Update' : 'Set'}
                    </button>
                    {key.isSet && key.source === 'runtime' && (
                      <button
                        onClick={() => handleClear(key.name)}
                        disabled={deleteApiKeyMutation.isPending}
                        className="px-3 py-1 text-xs bg-surface border border-red-500/30 text-red-400 rounded hover:bg-red-500/10 disabled:opacity-50 transition-colors"
                      >
                        Clear
                      </button>
                    )}
                  </>
                )}
              </div>
            </div>

            {key.isSet && editingKey !== key.name && (
              <p className="text-xs font-mono text-content-secondary">{key.maskedValue}</p>
            )}

            {!key.isSet && editingKey !== key.name && (
              <p className="text-xs text-content-secondary">Not configured</p>
            )}

            {editingKey === key.name && (
              <div className="flex items-center gap-2">
                <input
                  type="password"
                  value={keyValue}
                  onChange={(e) => setKeyValue(e.target.value)}
                  placeholder="sk-ant-..."
                  autoFocus
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') handleSave(key.name);
                    if (e.key === 'Escape') handleCancel();
                  }}
                  className="flex-1 px-3 py-1.5 text-sm font-mono bg-surface border border-edge rounded text-content placeholder:text-content-secondary/50 focus:outline-none focus:border-accent"
                />
                <button
                  onClick={() => handleSave(key.name)}
                  disabled={!keyValue.trim() || setApiKeyMutation.isPending}
                  className="px-3 py-1.5 text-xs bg-accent text-white rounded hover:bg-accent/90 disabled:opacity-50 transition-colors"
                >
                  {setApiKeyMutation.isPending ? 'Saving...' : 'Save'}
                </button>
                <button
                  onClick={handleCancel}
                  className="px-3 py-1.5 text-xs bg-surface border border-edge text-content rounded hover:bg-surface-alt transition-colors"
                >
                  Cancel
                </button>
              </div>
            )}
          </div>
        ))}
      </div>
    </Card>
  );
}

export function AdminTab() {
  const { data: status, isLoading, error } = useAdminStatus();
  const { phase, logs, durationMs, exitCode, startBuild, isBuilding } = useRebuild();
  const logEndRef = useRef<HTMLDivElement>(null);

  // Auto-scroll log to bottom
  useEffect(() => {
    logEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [logs.length]);

  if (isLoading) {
    return (
      <div className="flex items-center justify-center py-12">
        <Spinner size="lg" />
      </div>
    );
  }

  if (error) {
    return (
      <Card className="border-red-500/50 bg-red-500/10">
        <p className="text-red-400">Failed to load admin status: {String(error)}</p>
      </Card>
    );
  }

  return (
    <div className="flex flex-col gap-6">
      {/* Server status card */}
      <Card>
        <h2 className="text-lg font-semibold text-content mb-4">Server Status</h2>
        <div className="grid grid-cols-2 gap-4 text-sm">
          <div>
            <span className="text-content-secondary">Version</span>
            <p className="text-content font-mono">{status?.version}</p>
          </div>
          <div>
            <span className="text-content-secondary">Uptime</span>
            <p className="text-content">{status ? formatUptime(status.uptimeSeconds) : '-'}</p>
          </div>
          <div>
            <span className="text-content-secondary">Dev Mode</span>
            <div className="flex items-center gap-2">
              <span className={`inline-block w-2 h-2 rounded-full ${status?.devMode ? 'bg-green-500' : 'bg-red-500'}`} />
              <span className="text-content">{status?.devMode ? 'Enabled' : 'Disabled'}</span>
            </div>
          </div>
          <div>
            <span className="text-content-secondary">Status</span>
            <div className="text-content flex items-center gap-2">
              {isBuilding && <Spinner size="sm" />}
              {phaseLabel(phase)}
            </div>
          </div>
        </div>
      </Card>

      {/* API Keys card */}
      <ApiKeysCard />

      {/* Dev mode disabled hint */}
      {status && !status.devMode && (
        <Card className="border-yellow-500/30 bg-yellow-500/5">
          <p className="text-yellow-400 text-sm">
            Dev mode is not enabled. Start the server with{' '}
            <code className="bg-surface px-1.5 py-0.5 rounded text-xs">AIDA_DEV_MODE=1</code>{' '}
            to enable rebuild and restart from the UI.
          </p>
        </Card>
      )}

      {/* Build actions */}
      {status?.devMode && (
        <Card>
          <h2 className="text-lg font-semibold text-content mb-4">Build Actions</h2>
          <div className="flex gap-3">
            <button
              onClick={() => startBuild(true)}
              disabled={isBuilding}
              className="px-4 py-2 bg-accent text-white rounded-lg text-sm font-medium hover:bg-accent/90 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
            >
              Rebuild & Restart
            </button>
            <button
              onClick={() => startBuild(false)}
              disabled={isBuilding}
              className="px-4 py-2 bg-surface border border-edge text-content rounded-lg text-sm font-medium hover:bg-surface-alt disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
            >
              Build Only
            </button>
          </div>
          {durationMs != null && phase !== 'building' && (
            <p className="text-xs text-content-secondary mt-2">
              Completed in {(durationMs / 1000).toFixed(1)}s
              {exitCode != null && exitCode !== 0 && ` (exit code: ${exitCode})`}
            </p>
          )}
        </Card>
      )}

      {/* Build error banner */}
      {phase === 'failed' && (
        <Card className="border-red-500/50 bg-red-500/10">
          <p className="text-red-400 font-medium">Build failed{exitCode != null ? ` with exit code ${exitCode}` : ''}</p>
        </Card>
      )}

      {/* Reconnecting banner */}
      {phase === 'reconnecting' && (
        <Card className="border-blue-500/50 bg-blue-500/10">
          <div className="text-blue-400 flex items-center gap-2">
            <Spinner size="sm" />
            Server is restarting. The page will reload automatically when the server is back...
          </div>
        </Card>
      )}

      {/* Terminal log */}
      {logs.length > 0 && (
        <div>
          <div className="flex items-center justify-between mb-2">
            <h3 className="text-sm font-medium text-content-secondary">Build Output</h3>
            <span className="text-xs text-content-secondary">{logs.length} lines</span>
          </div>
          <div className="bg-gray-950 rounded-lg border border-edge p-4 max-h-96 overflow-y-auto font-mono text-xs leading-5">
            {logs.map((entry, i) => (
              <div
                key={i}
                className={entry.stream === 'stderr' ? 'text-yellow-400' : 'text-gray-400'}
              >
                {entry.line}
              </div>
            ))}
            <div ref={logEndRef} />
          </div>
        </div>
      )}
    </div>
  );
}
