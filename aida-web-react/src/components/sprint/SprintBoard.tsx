import type { Requirement } from '@shared/types';
import { SprintColumn } from './SprintColumn';

interface SprintBoardProps {
  backlog: Requirement[];
  sprintItems: Requirement[];
  sprintId: string;
  sprintTitle: string;
}

export function SprintBoard({ backlog, sprintItems, sprintId, sprintTitle }: SprintBoardProps) {
  return (
    <div className="flex gap-4 min-h-[400px]">
      <SprintColumn
        id="backlog"
        title="Backlog"
        items={backlog}
        accent="bg-gray-400"
      />
      <SprintColumn
        id={sprintId}
        title={sprintTitle}
        items={sprintItems}
        accent="bg-accent"
      />
    </div>
  );
}
