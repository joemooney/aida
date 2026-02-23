// trace:TASK-0001 | ai:claude
import { useCallback, useRef, useState } from 'react';
import { runSkill, executeSkillAction } from '../api/skillRunner';
import type { WarningsReport } from '../api/skillRunner';

export type SkillPhase = 'idle' | 'running' | 'done' | 'error';

export interface LogEntry {
  line: string;
  stream: 'stdout' | 'stderr';
  timestamp: number;
}

export function useSkillRunner() {
  const [phase, setPhase] = useState<SkillPhase>('idle');
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [result, setResult] = useState<WarningsReport | null>(null);
  const [progressText, setProgressText] = useState('');
  const [errorMessage, setErrorMessage] = useState('');
  const abortRef = useRef<AbortController | null>(null);

  const run = useCallback(async (skillName: string) => {
    // Reset state
    setLogs([]);
    setResult(null);
    setPhase('running');
    setProgressText('Starting...');
    setErrorMessage('');

    try {
      const response = await runSkill(skillName);
      const reader = response.body?.getReader();
      if (!reader) throw new Error('No response body');

      const decoder = new TextDecoder();
      let buffer = '';

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        buffer += decoder.decode(value, { stream: true });

        // Parse SSE events from buffer
        while (true) {
          const eventEnd = buffer.indexOf('\n\n');
          if (eventEnd === -1) break;

          const eventStr = buffer.slice(0, eventEnd);
          buffer = buffer.slice(eventEnd + 2);

          let eventType = '';
          let data = '';

          for (const line of eventStr.split('\n')) {
            if (line.startsWith('event: ')) {
              eventType = line.slice(7);
            } else if (line.startsWith('data: ')) {
              data = line.slice(6);
            }
          }

          if (eventType === 'log') {
            try {
              const parsed = JSON.parse(data);
              setLogs((prev) => [
                ...prev,
                { line: parsed.line, stream: parsed.stream, timestamp: Date.now() },
              ]);
            } catch {
              // skip malformed
            }
          } else if (eventType === 'progress') {
            try {
              const parsed = JSON.parse(data);
              setProgressText(parsed.phase || '');
            } catch {
              // skip
            }
          } else if (eventType === 'result') {
            try {
              const parsed: WarningsReport = JSON.parse(data);
              setResult(parsed);
            } catch {
              // skip
            }
          } else if (eventType === 'error') {
            try {
              const parsed = JSON.parse(data);
              setErrorMessage(parsed.message || 'Unknown error');
              setPhase('error');
            } catch {
              setErrorMessage(data || 'Unknown error');
              setPhase('error');
            }
          } else if (eventType === 'done') {
            // Only set done if we're not already in error state
            setPhase((prev) => (prev === 'error' ? 'error' : 'done'));
          }
        }
      }

      // If stream ended without explicit done/error, mark as done
      setPhase((prev) => (prev === 'running' ? 'done' : prev));
    } catch (err) {
      setErrorMessage(err instanceof Error ? err.message : 'Unknown error');
      setPhase('error');
    }
  }, []);

  const executeAction = useCallback(
    async (skillName: string, action: string, params: Record<string, unknown>) => {
      return executeSkillAction(skillName, action, params);
    },
    [],
  );

  const reset = useCallback(() => {
    abortRef.current?.abort();
    abortRef.current = null;
    setPhase('idle');
    setLogs([]);
    setResult(null);
    setProgressText('');
    setErrorMessage('');
  }, []);

  return {
    phase,
    logs,
    result,
    progressText,
    errorMessage,
    run,
    reset,
    executeAction,
    isRunning: phase === 'running',
  };
}
