import { useMutation, useQueryClient } from '@tanstack/react-query';
import { assignToSprint, removeFromSprint, createSprint, type CreateSprintData } from '../api/sprints';

export function useCreateSprint() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (data: CreateSprintData) => createSprint(data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
    },
  });
}

export function useAssignToSprint() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ reqId, sprintId }: { reqId: string; sprintId: string }) =>
      assignToSprint(reqId, sprintId),
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
    },
  });
}

export function useRemoveFromSprint() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (reqId: string) => removeFromSprint(reqId),
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
    },
  });
}
