import { useMutation, useQueryClient } from '@tanstack/react-query';
import { assignToSprint, removeFromSprint, createSprint, type CreateSprintData } from '../api/sprints';
import { requireWrite, usePermissions } from './usePermissions';

export function useCreateSprint() {
  const { canWrite } = usePermissions();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (data: CreateSprintData) => {
      requireWrite(canWrite);
      return createSprint(data);
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
    },
  });
}

export function useAssignToSprint() {
  const { canWrite } = usePermissions();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ reqId, sprintId }: { reqId: string; sprintId: string }) => {
      requireWrite(canWrite);
      return assignToSprint(reqId, sprintId);
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
    },
  });
}

export function useRemoveFromSprint() {
  const { canWrite } = usePermissions();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (reqId: string) => {
      requireWrite(canWrite);
      return removeFromSprint(reqId);
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
    },
  });
}
