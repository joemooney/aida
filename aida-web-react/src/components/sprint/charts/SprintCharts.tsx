import { useMemo } from 'react';
import type { Requirement } from '@shared/types';
import {
  computeBurndownData,
  computeBurnupData,
  computeVelocityData,
  getSprintDates,
} from '../../../lib/sprint-utils';
import { BurndownChart } from './BurndownChart';
import { BurnupChart } from './BurnupChart';
import { VelocityChart } from './VelocityChart';

interface SprintChartsProps {
  selectedSprint: Requirement;
  sprintItems: Requirement[];
  allSprints: Requirement[];
  sprintItemsMap: Record<string, Requirement[]>;
}

export function SprintCharts({ selectedSprint, sprintItems, allSprints, sprintItemsMap }: SprintChartsProps) {
  const { start, end } = getSprintDates(selectedSprint);

  const burndownData = useMemo(
    () => (start && end ? computeBurndownData(sprintItems, start, end) : []),
    [sprintItems, start, end],
  );

  const burnupData = useMemo(
    () => (start && end ? computeBurnupData(sprintItems, start, end) : []),
    [sprintItems, start, end],
  );

  const velocityData = useMemo(
    () => computeVelocityData(allSprints, sprintItemsMap),
    [allSprints, sprintItemsMap],
  );

  return (
    <div className="grid grid-cols-1 md:grid-cols-3 gap-4 rounded-xl border border-edge bg-surface-alt p-4">
      <BurndownChart data={burndownData} />
      <BurnupChart data={burnupData} />
      <VelocityChart data={velocityData} />
    </div>
  );
}
