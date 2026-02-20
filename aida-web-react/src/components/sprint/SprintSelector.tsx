import type { Requirement } from '@shared/types';
import { SprintCard } from './SprintCard';

interface SprintSelectorProps {
  sprints: Requirement[];
  sprintItemsMap: Record<string, Requirement[]>;
  selectedId: string | null;
  onSelect: (id: string) => void;
}

export function SprintSelector({ sprints, sprintItemsMap, selectedId, onSelect }: SprintSelectorProps) {
  if (sprints.length === 0) return null;

  return (
    <div className="flex gap-3 overflow-x-auto pb-2 -mx-1 px-1">
      {sprints.map((sprint) => (
        <SprintCard
          key={sprint.id}
          sprint={sprint}
          items={sprintItemsMap[sprint.id] ?? []}
          selected={sprint.id === selectedId}
          onClick={() => onSelect(sprint.id)}
        />
      ))}
    </div>
  );
}
