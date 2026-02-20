import type { Requirement, Relationship } from '@shared/types';

export function isSprintAssignment(rel: Relationship): boolean {
  return (
    typeof rel.rel_type === 'object' &&
    'Custom' in rel.rel_type &&
    rel.rel_type.Custom === 'sprint_assignment'
  );
}

export function getSprintNumber(sprint: Requirement): number | null {
  const val = sprint.custom_fields?.sprint_number;
  if (!val) return null;
  const num = Number(val);
  return isNaN(num) ? null : num;
}

export function getSprintGoal(sprint: Requirement): string {
  return sprint.custom_fields?.sprint_goal ?? '';
}

export function getSprintDates(sprint: Requirement): { start: string | null; end: string | null } {
  return {
    start: sprint.custom_fields?.start_date ?? null,
    end: sprint.custom_fields?.end_date ?? null,
  };
}

export type SprintState = 'active' | 'past' | 'future' | 'unknown';

export function getSprintState(sprint: Requirement): SprintState {
  const { start, end } = getSprintDates(sprint);
  if (!start || !end) return 'unknown';

  const now = new Date();
  const startDate = new Date(start);
  const endDate = new Date(end);

  if (now < startDate) return 'future';
  if (now > endDate) return 'past';
  return 'active';
}

export interface SprintProgress {
  total: number;
  completed: number;
  percentage: number;
  totalPoints: number;
  completedPoints: number;
}

export function computeSprintProgress(items: Requirement[]): SprintProgress {
  let totalPoints = 0;
  let completedPoints = 0;
  let completed = 0;

  for (const item of items) {
    const points = item.weight ?? 0;
    totalPoints += points;
    if (item.status === 'Completed') {
      completed++;
      completedPoints += points;
    }
  }

  return {
    total: items.length,
    completed,
    percentage: items.length > 0 ? Math.round((completed / items.length) * 100) : 0,
    totalPoints,
    completedPoints,
  };
}

export function getSprintAssignmentTarget(req: Requirement): string | null {
  const rel = req.relationships?.find(isSprintAssignment);
  return rel ? rel.target_id : null;
}
