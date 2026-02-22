import { useMutation, useQueryClient } from '@tanstack/react-query';
import { evaluateRequirement } from '../api/evaluate';

export function useEvaluateRequirement() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (id: string) => evaluateRequirement(id),
    onSuccess: (_data, id) => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
      queryClient.invalidateQueries({ queryKey: ['requirement', id] });
    },
  });
}
