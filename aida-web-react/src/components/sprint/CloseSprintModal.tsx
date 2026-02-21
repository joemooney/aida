import { useState, useEffect, useCallback } from 'react';
import { X, CheckCircle2 } from 'lucide-react';
import type { Requirement } from '@shared/types';
import { useUpdateRequirement } from '../../hooks/useRequirements';
import { useCreateSprint, useAssignToSprint } from '../../hooks/useSprints';
import {
  getSprintNumber,
  getSprintDates,
  computeSprintProgress,
} from '../../lib/sprint-utils';
import { formatDate } from '../../lib/utils';
import { SprintProgressBar } from './SprintProgressBar';

interface CloseSprintModalProps {
  sprint: Requirement;
  items: Requirement[];
  nextSprintNumber: number;
  onClose: () => void;
}

// trace:TASK-sprint-close | ai:claude
export function CloseSprintModal({ sprint, items, nextSprintNumber, onClose }: CloseSprintModalProps) {
  const updateMutation = useUpdateRequirement();
  const createMutation = useCreateSprint();
  const assignMutation = useAssignToSprint();

  const incompleteItems = items.filter((i) => i.status !== 'Completed');
  const [checkedIds, setCheckedIds] = useState<Set<string>>(
    () => new Set(incompleteItems.map((i) => i.id)),
  );
  const [isClosing, setIsClosing] = useState(false);
  const [stepError, setStepError] = useState<string | null>(null);

  const progress = computeSprintProgress(items);
  const num = getSprintNumber(sprint);
  const { start, end } = getSprintDates(sprint);

  const isBusy = isClosing;

  const handleClose = useCallback(() => {
    if (!isBusy) onClose();
  }, [isBusy, onClose]);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') handleClose();
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [handleClose]);

  const toggleItem = (id: string) => {
    setCheckedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const toggleAll = () => {
    if (checkedIds.size === incompleteItems.length) {
      setCheckedIds(new Set());
    } else {
      setCheckedIds(new Set(incompleteItems.map((i) => i.id)));
    }
  };

  const closeSprint = async () => {
    const sprintId = sprint.spec_id ?? sprint.id;
    await updateMutation.mutateAsync({
      id: sprintId,
      data: { status: 'Completed' },
    });
  };

  const handleCloseSprint = async () => {
    setIsClosing(true);
    setStepError(null);
    try {
      await closeSprint();
      onClose();
    } catch {
      setStepError('Failed to close sprint.');
    } finally {
      setIsClosing(false);
    }
  };

  const handleCloseAndCreateNext = async () => {
    setIsClosing(true);
    setStepError(null);

    try {
      // Step 1: Close current sprint
      await closeSprint();
    } catch {
      setStepError('Failed to close sprint.');
      setIsClosing(false);
      return;
    }

    // Step 2: Create next sprint
    let newSprint: Requirement;
    try {
      // Calculate next sprint dates: day after current end + 2 weeks
      let nextStart = '';
      let nextEnd = '';
      if (end) {
        const endDate = new Date(end);
        const startDate = new Date(endDate);
        startDate.setDate(startDate.getDate() + 1);
        const endDateNext = new Date(startDate);
        endDateNext.setDate(endDateNext.getDate() + 13); // 2 weeks
        nextStart = startDate.toISOString().slice(0, 10);
        nextEnd = endDateNext.toISOString().slice(0, 10);
      }

      newSprint = await createMutation.mutateAsync({
        title: `Sprint ${nextSprintNumber}`,
        sprint_number: String(nextSprintNumber),
        start_date: nextStart,
        end_date: nextEnd,
      });
    } catch {
      setStepError('Sprint closed, but failed to create next sprint.');
      setIsClosing(false);
      return;
    }

    // Step 3: Move checked incomplete items to new sprint
    const itemsToMove = incompleteItems.filter((i) => checkedIds.has(i.id));
    let moveErrors = 0;
    for (const item of itemsToMove) {
      try {
        await assignMutation.mutateAsync({
          reqId: item.spec_id ?? item.id,
          sprintId: newSprint.id,
        });
      } catch {
        moveErrors++;
      }
    }

    if (moveErrors > 0) {
      setStepError(`Sprint closed and next created, but ${moveErrors} item(s) failed to move.`);
      setIsClosing(false);
      return;
    }

    onClose();
  };

  const sprintLabel = num != null ? `Sprint ${num}` : sprint.title;

  return (
    <>
      {/* Backdrop */}
      <div
        className="fixed inset-0 bg-black/40 z-40 animate-fade-in"
        onClick={handleClose}
      />

      {/* Modal */}
      <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
        <div
          className="bg-surface-alt border border-edge rounded-2xl shadow-2xl shadow-black/40 w-full max-w-lg flex flex-col gap-5 p-6 animate-fade-in"
          onClick={(e) => e.stopPropagation()}
        >
          {/* Header */}
          <div className="flex items-center justify-between">
            <h2 className="text-lg font-semibold text-content">Close Sprint</h2>
            <button
              type="button"
              onClick={handleClose}
              disabled={isBusy}
              className="p-1 rounded-lg text-content-muted hover:text-content hover:bg-surface-hover transition-colors disabled:opacity-50"
            >
              <X className="h-5 w-5" />
            </button>
          </div>

          {/* Summary */}
          <div className="rounded-xl border border-edge bg-surface p-4 flex flex-col gap-3">
            <div className="flex items-center justify-between">
              <span className="text-sm font-semibold text-content">{sprintLabel}</span>
              {start && end && (
                <span className="text-xs text-content-muted">
                  {formatDate(start)} - {formatDate(end)}
                </span>
              )}
            </div>
            <SprintProgressBar
              percentage={progress.percentage}
              label={`${progress.completed}/${progress.total} items completed${progress.totalPoints > 0 ? ` · ${progress.completedPoints}/${progress.totalPoints} pts` : ''}`}
            />
          </div>

          {/* Incomplete items */}
          {incompleteItems.length > 0 && (
            <div className="flex flex-col gap-2">
              <div className="flex items-center justify-between">
                <span className="text-xs font-medium text-content-secondary">
                  Incomplete Items ({incompleteItems.length})
                </span>
                <button
                  type="button"
                  onClick={toggleAll}
                  className="text-xs text-accent hover:text-accent/80 transition-colors"
                >
                  {checkedIds.size === incompleteItems.length ? 'Deselect all' : 'Select all'}
                </button>
              </div>
              <div className="max-h-48 overflow-y-auto rounded-lg border border-edge bg-surface divide-y divide-edge">
                {incompleteItems.map((item) => (
                  <label
                    key={item.id}
                    className="flex items-center gap-3 px-3 py-2 hover:bg-surface-hover transition-colors cursor-pointer"
                  >
                    <input
                      type="checkbox"
                      checked={checkedIds.has(item.id)}
                      onChange={() => toggleItem(item.id)}
                      className="rounded border-edge text-accent focus:ring-accent"
                    />
                    <div className="flex flex-col min-w-0 flex-1">
                      <span className="text-sm text-content truncate">{item.title}</span>
                      <span className="text-[11px] text-content-muted">
                        {item.spec_id} · {item.status}
                      </span>
                    </div>
                  </label>
                ))}
              </div>
              <p className="text-[11px] text-content-muted">
                Checked items will be carried over to the next sprint when using "Close &amp; Create Next".
              </p>
            </div>
          )}

          {incompleteItems.length === 0 && (
            <div className="flex items-center gap-2 rounded-lg bg-emerald-500/10 px-3 py-2">
              <CheckCircle2 className="h-4 w-4 text-emerald-400 shrink-0" />
              <span className="text-sm text-emerald-400">All items completed!</span>
            </div>
          )}

          {/* Error */}
          {stepError && (
            <p className="text-xs text-red-400">{stepError}</p>
          )}

          {/* Actions */}
          <div className="flex justify-end gap-2 pt-1">
            <button
              type="button"
              onClick={handleClose}
              disabled={isBusy}
              className="rounded-lg px-4 py-2 text-sm font-medium text-content-secondary hover:bg-surface-hover transition-colors disabled:opacity-50"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={handleCloseSprint}
              disabled={isBusy}
              className="rounded-lg border border-edge px-4 py-2 text-sm font-medium text-content hover:bg-surface-hover transition-colors disabled:opacity-50"
            >
              {isClosing ? 'Closing...' : 'Close Sprint'}
            </button>
            {incompleteItems.length > 0 && (
              <button
                type="button"
                onClick={handleCloseAndCreateNext}
                disabled={isBusy || checkedIds.size === 0}
                className="rounded-lg bg-accent px-4 py-2 text-sm font-medium text-white hover:bg-accent/90 disabled:opacity-50 transition-colors"
              >
                {isClosing ? 'Processing...' : 'Close & Create Next'}
              </button>
            )}
          </div>
        </div>
      </div>
    </>
  );
}
