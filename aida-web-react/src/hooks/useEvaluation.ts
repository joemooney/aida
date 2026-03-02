import { useMutation, useQueryClient } from '@tanstack/react-query';
import { evaluateRequirement } from '../api/evaluate';
import { requireWrite, usePermissions } from './usePermissions';

export function useEvaluateRequirement() {
  const { canWrite } = usePermissions();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (id: string) => {
      requireWrite(canWrite);
      return evaluateRequirement(id);
    },
    onSuccess: (_data, id) => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
      queryClient.invalidateQueries({ queryKey: ['requirement', id] });
    },
  });
}
