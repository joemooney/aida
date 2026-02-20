import { useState, useEffect, useCallback } from 'react';
import { X } from 'lucide-react';
import { useCreateSprint } from '../../hooks/useSprints';

interface CreateSprintModalProps {
  nextSprintNumber: number;
  onClose: () => void;
}

export function CreateSprintModal({ nextSprintNumber, onClose }: CreateSprintModalProps) {
  const createMutation = useCreateSprint();
  const [sprintNumber, setSprintNumber] = useState(String(nextSprintNumber));
  const [title, setTitle] = useState(`Sprint ${nextSprintNumber}`);
  const [startDate, setStartDate] = useState('');
  const [endDate, setEndDate] = useState('');
  const [goal, setGoal] = useState('');
  const [plannedVelocity, setPlannedVelocity] = useState('');

  // Sync title when sprint number changes
  useEffect(() => {
    const num = Number(sprintNumber);
    if (!isNaN(num)) {
      setTitle(`Sprint ${num}`);
    }
  }, [sprintNumber]);

  const handleClose = useCallback(() => {
    if (!createMutation.isPending) onClose();
  }, [createMutation.isPending, onClose]);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') handleClose();
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [handleClose]);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    createMutation.mutate(
      {
        title,
        sprint_number: sprintNumber,
        start_date: startDate,
        end_date: endDate,
        sprint_goal: goal || undefined,
        planned_velocity: plannedVelocity || undefined,
      },
      { onSuccess: onClose },
    );
  };

  return (
    <>
      {/* Backdrop */}
      <div
        className="fixed inset-0 bg-black/40 z-40 animate-fade-in"
        onClick={handleClose}
      />

      {/* Modal */}
      <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
        <form
          onSubmit={handleSubmit}
          className="bg-surface-alt border border-edge rounded-2xl shadow-2xl shadow-black/40 w-full max-w-md flex flex-col gap-5 p-6 animate-fade-in"
          onClick={(e) => e.stopPropagation()}
        >
          {/* Header */}
          <div className="flex items-center justify-between">
            <h2 className="text-lg font-semibold text-content">New Sprint</h2>
            <button
              type="button"
              onClick={handleClose}
              className="p-1 rounded-lg text-content-muted hover:text-content hover:bg-surface-hover transition-colors"
            >
              <X className="h-5 w-5" />
            </button>
          </div>

          {/* Fields */}
          <div className="flex flex-col gap-4">
            <div className="flex gap-3">
              <label className="flex flex-col gap-1 flex-1">
                <span className="text-xs font-medium text-content-secondary">Sprint Number</span>
                <input
                  type="number"
                  min={1}
                  required
                  value={sprintNumber}
                  onChange={(e) => setSprintNumber(e.target.value)}
                  className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none"
                />
              </label>

              <label className="flex flex-col gap-1 flex-[2]">
                <span className="text-xs font-medium text-content-secondary">Title</span>
                <input
                  type="text"
                  required
                  value={title}
                  onChange={(e) => setTitle(e.target.value)}
                  className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none"
                />
              </label>
            </div>

            <div className="flex gap-3">
              <label className="flex flex-col gap-1 flex-1">
                <span className="text-xs font-medium text-content-secondary">Start Date</span>
                <input
                  type="date"
                  required
                  value={startDate}
                  onChange={(e) => setStartDate(e.target.value)}
                  className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
                />
              </label>
              <label className="flex flex-col gap-1 flex-1">
                <span className="text-xs font-medium text-content-secondary">End Date</span>
                <input
                  type="date"
                  required
                  value={endDate}
                  onChange={(e) => setEndDate(e.target.value)}
                  className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
                />
              </label>
            </div>

            <label className="flex flex-col gap-1">
              <span className="text-xs font-medium text-content-secondary">Sprint Goal</span>
              <input
                type="text"
                value={goal}
                onChange={(e) => setGoal(e.target.value)}
                placeholder="What should this sprint accomplish?"
                className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none"
              />
            </label>

            <label className="flex flex-col gap-1 max-w-[140px]">
              <span className="text-xs font-medium text-content-secondary">Planned Velocity</span>
              <input
                type="number"
                min={0}
                value={plannedVelocity}
                onChange={(e) => setPlannedVelocity(e.target.value)}
                placeholder="pts"
                className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none"
              />
            </label>
          </div>

          {/* Error */}
          {createMutation.isError && (
            <p className="text-xs text-red-400">
              Failed to create sprint. Please try again.
            </p>
          )}

          {/* Actions */}
          <div className="flex justify-end gap-2 pt-1">
            <button
              type="button"
              onClick={handleClose}
              className="rounded-lg px-4 py-2 text-sm font-medium text-content-secondary hover:bg-surface-hover transition-colors"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={createMutation.isPending}
              className="rounded-lg bg-accent px-4 py-2 text-sm font-medium text-white hover:bg-accent/90 disabled:opacity-50 transition-colors"
            >
              {createMutation.isPending ? 'Creating...' : 'Create Sprint'}
            </button>
          </div>
        </form>
      </div>
    </>
  );
}
