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

// ============================================================================
// Chart data computation
// ============================================================================

export interface BurndownPoint {
  date: string;
  remaining: number;
  ideal: number;
}

/** Compute burndown data by scanning item history for status → Completed transitions. */
export function computeBurndownData(
  items: Requirement[],
  startDate: string,
  endDate: string,
): BurndownPoint[] {
  const start = new Date(startDate);
  const end = new Date(endDate);
  const total = items.length;
  if (total === 0 || start >= end) return [];

  // Build a map of date → count of items completed on that date
  const completionsByDate = new Map<string, number>();
  for (const item of items) {
    if (item.status !== 'Completed') continue;
    let completedDate: string | null = null;
    // Scan history for status change to Completed
    for (const entry of item.history ?? []) {
      for (const change of entry.changes) {
        if (change.field_name === 'status' && change.new_value === 'Completed') {
          completedDate = entry.timestamp.slice(0, 10);
        }
      }
    }
    // Fallback: use modified_at
    if (!completedDate) {
      completedDate = item.modified_at.slice(0, 10);
    }
    completionsByDate.set(completedDate, (completionsByDate.get(completedDate) ?? 0) + 1);
  }

  const days = daysBetween(start, end);
  const points: BurndownPoint[] = [];
  let remaining = total;

  for (let i = 0; i <= days; i++) {
    const d = new Date(start);
    d.setDate(d.getDate() + i);
    const dateStr = d.toISOString().slice(0, 10);
    const ideal = total - (total * i) / days;

    remaining -= completionsByDate.get(dateStr) ?? 0;
    points.push({ date: dateStr, remaining, ideal: Math.round(ideal * 10) / 10 });
  }

  return points;
}

export interface BurnupPoint {
  date: string;
  completed: number;
  scope: number;
}

/** Compute burn-up data: cumulative completed items and scope line. */
export function computeBurnupData(
  items: Requirement[],
  startDate: string,
  endDate: string,
): BurnupPoint[] {
  const start = new Date(startDate);
  const end = new Date(endDate);
  if (items.length === 0 || start >= end) return [];

  const completionsByDate = new Map<string, number>();
  for (const item of items) {
    if (item.status !== 'Completed') continue;
    let completedDate: string | null = null;
    for (const entry of item.history ?? []) {
      for (const change of entry.changes) {
        if (change.field_name === 'status' && change.new_value === 'Completed') {
          completedDate = entry.timestamp.slice(0, 10);
        }
      }
    }
    if (!completedDate) completedDate = item.modified_at.slice(0, 10);
    completionsByDate.set(completedDate, (completionsByDate.get(completedDate) ?? 0) + 1);
  }

  const days = daysBetween(start, end);
  const points: BurnupPoint[] = [];
  let cumCompleted = 0;

  for (let i = 0; i <= days; i++) {
    const d = new Date(start);
    d.setDate(d.getDate() + i);
    const dateStr = d.toISOString().slice(0, 10);
    cumCompleted += completionsByDate.get(dateStr) ?? 0;
    points.push({ date: dateStr, completed: cumCompleted, scope: items.length });
  }

  return points;
}

export interface VelocityPoint {
  sprintLabel: string;
  points: number;
}

/** Compute velocity data for all sprints. */
export function computeVelocityData(
  sprints: Requirement[],
  sprintItemsMap: Record<string, Requirement[]>,
): VelocityPoint[] {
  return sprints.map((sprint) => {
    const items = sprintItemsMap[sprint.id] ?? [];
    const num = getSprintNumber(sprint);
    const completedPoints = items
      .filter((i) => i.status === 'Completed')
      .reduce((sum, i) => sum + (i.weight ?? 1), 0);
    return {
      sprintLabel: num != null ? `S${num}` : sprint.title.slice(0, 8),
      points: completedPoints,
    };
  });
}

function daysBetween(a: Date, b: Date): number {
  return Math.max(0, Math.round((b.getTime() - a.getTime()) / (1000 * 60 * 60 * 24)));
}

export function getPlannedVelocity(sprint: Requirement): number | null {
  const val = sprint.custom_fields?.planned_velocity;
  if (!val) return null;
  const num = Number(val);
  return isNaN(num) ? null : num;
}
