// trace:TASK-0001 | ai:claude
import { useEffect, useRef, useState } from 'react';
import { X, Play, RotateCcw, Terminal, ChevronDown, ChevronRight } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useSkillRunner, type LogEntry } from '../../hooks/useSkillRunner';
import { WarningsReportView } from './WarningsReport';
import { Spinner } from '../ui/Spinner';
import { SkillChat } from './SkillChat';

interface SkillRunnerPanelProps {
  skillName: string;
  skillDescription: string;
  onClose: () => void;
}

export function SkillRunnerPanel({ skillName, skillDescription, onClose }: SkillRunnerPanelProps) {
  const {
    phase,
    logs,
    result,
    progressText,
    errorMessage,
    run,
    reset,
    executeAction,
    isRunning,
  } = useSkillRunner();

  const [actionPending, setActionPending] = useState(false);
  const [actionMessage, setActionMessage] = useState('');
  const [logsExpanded, setLogsExpanded] = useState(true);
  const logEndRef = useRef<HTMLDivElement>(null);

  // Auto-scroll log
  useEffect(() => {
    logEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [logs.length]);

  // Collapse logs when results arrive
  useEffect(() => {
    if (result) setLogsExpanded(false);
  }, [result]);

  // Close on Escape
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') onClose();
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [onClose]);

  const handleRun = () => {
    run(skillName);
  };

  const handleAction = async (action: string, params: Record<string, unknown>) => {
    setActionPending(true);
    setActionMessage('');
    try {
      const response = await executeAction(skillName, action, params);
      setActionMessage(response.message);
    } catch (err) {
      setActionMessage(`Error: ${err instanceof Error ? err.message : 'Unknown error'}`);
    } finally {
      setActionPending(false);
    }
  };

  const handleReset = () => {
    reset();
    setActionMessage('');
  };

  return (
    <>
      {/* Backdrop */}
      <div
        className="fixed inset-0 bg-black/40 z-40 animate-fade-in"
        onClick={onClose}
      />

      {/* Panel */}
      <div className="fixed top-0 right-0 bottom-0 z-50 w-full max-w-3xl bg-surface-alt border-l border-edge flex flex-col animate-slide-in-right shadow-2xl shadow-black/40">
        {/* Header */}
        <div className="flex items-center justify-between border-b border-edge px-5 py-4">
          <div className="flex items-center gap-3 min-w-0">
            <Terminal className="h-4 w-4 text-accent shrink-0" />
            <h2 className="text-base font-semibold text-content truncate">{skillName}</h2>
            {isRunning && <Spinner size="sm" />}
          </div>
          <div className="flex items-center gap-2">
            {phase === 'idle' && (
              <button
                onClick={handleRun}
                className="inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium bg-accent text-white hover:bg-accent-hover transition-colors cursor-pointer"
              >
                <Play className="h-3.5 w-3.5" />
                Run
              </button>
            )}
            {(phase === 'done' || phase === 'error') && (
              <button
                onClick={handleReset}
                className="inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium text-content-secondary hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
              >
                <RotateCcw className="h-3.5 w-3.5" />
                Reset
              </button>
            )}
            {(phase === 'done' || phase === 'error') && (
              <button
                onClick={handleRun}
                className="inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium bg-accent text-white hover:bg-accent-hover transition-colors cursor-pointer"
              >
                <Play className="h-3.5 w-3.5" />
                Re-Run
              </button>
            )}
            <button
              onClick={onClose}
              className="rounded-lg p-1.5 text-content-muted hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
        </div>

        {/* Description */}
        <div className="px-5 py-2.5 text-xs text-content-secondary border-b border-edge">
          {skillDescription}
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto px-5 py-4 space-y-4">
          {/* Progress indicator */}
          {isRunning && progressText && (
            <div className="flex items-center gap-2 text-sm text-accent">
              <Spinner size="sm" />
              {progressText}
            </div>
          )}

          {/* Error banner */}
          {phase === 'error' && errorMessage && (
            <div className="rounded-lg border border-red-500/50 bg-red-500/10 p-4">
              <p className="text-red-400 text-sm font-medium">Error</p>
              <p className="text-red-300 text-xs mt-1">{errorMessage}</p>
            </div>
          )}

          {/* Action feedback */}
          {actionMessage && (
            <div className={cn(
              'rounded-lg border p-3 text-xs',
              actionMessage.startsWith('Error')
                ? 'border-red-500/50 bg-red-500/10 text-red-300'
                : 'border-emerald-500/50 bg-emerald-500/10 text-emerald-300',
            )}>
              {actionMessage}
            </div>
          )}

          {/* Log output */}
          {logs.length > 0 && (
            <div>
              <button
                onClick={() => setLogsExpanded((e) => !e)}
                className="flex items-center gap-2 mb-2 cursor-pointer"
              >
                {logsExpanded ? (
                  <ChevronDown className="h-3.5 w-3.5 text-content-muted" />
                ) : (
                  <ChevronRight className="h-3.5 w-3.5 text-content-muted" />
                )}
                <h3 className="text-sm font-medium text-content-secondary">Build Output</h3>
                <span className="text-xs text-content-muted">({logs.length} lines)</span>
              </button>
              {logsExpanded && (
                <LogOutput logs={logs} logEndRef={logEndRef} />
              )}
            </div>
          )}

          {/* Results */}
          {result && (
            <div>
              <h3 className="text-sm font-medium text-content mb-3">Results</h3>
              <WarningsReportView
                report={result}
                onAction={handleAction}
                actionPending={actionPending}
              />
            </div>
          )}

          {/* Idle state */}
          {phase === 'idle' && (
            <div className="flex flex-col items-center justify-center py-16 text-content-muted">
              <Terminal className="h-12 w-12 mb-4 opacity-40" />
              <p className="text-sm font-medium">Ready to run</p>
              <p className="text-xs mt-1">Click "Run" to execute this skill</p>
            </div>
          )}

          {/* Chat integration (Phase 3) */}
          {result && phase === 'done' && (
            <SkillChat skillName={skillName} warningsReport={result} />
          )}
        </div>
      </div>
    </>
  );
}

function LogOutput({
  logs,
  logEndRef,
}: {
  logs: LogEntry[];
  logEndRef: React.RefObject<HTMLDivElement | null>;
}) {
  return (
    <div className="bg-gray-950 rounded-lg border border-edge p-4 max-h-64 overflow-y-auto font-mono text-xs leading-5">
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
  );
}
