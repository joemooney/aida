import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import type { Requirement } from '@shared/types';
import { fetchRequirements, fetchRequirement, updateRequirement, createRequirement, setParent } from '../api/requirements';

export function useRequirements() {
  return useQuery<Requirement[]>({
    queryKey: ['requirements'],
    queryFn: fetchRequirements,
    staleTime: 30_000,
  });
}

export function useRequirement(id: string | null) {
  return useQuery<Requirement>({
    queryKey: ['requirement', id],
    queryFn: () => fetchRequirement(id!),
    enabled: !!id,
  });
}

export function useUpdateRequirement() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ id, data }: { id: string; data: Partial<Requirement> }) =>
      updateRequirement(id, data),
    onMutate: async ({ id, data }) => {
      await queryClient.cancelQueries({ queryKey: ['requirements'] });
      const previous = queryClient.getQueryData<Requirement[]>(['requirements']);

      queryClient.setQueryData<Requirement[]>(['requirements'], (old) =>
        old?.map((req) =>
          req.id === id || req.spec_id === id ? { ...req, ...data } : req,
        ),
      );

      return { previous };
    },
    onError: (_err, _vars, context) => {
      if (context?.previous) {
        queryClient.setQueryData(['requirements'], context.previous);
      }
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
    },
  });
}

export function useCreateRequirement() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (data: Partial<Requirement>) => createRequirement(data),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
    },
  });
}

export function useSetParent() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, parentId }: { id: string; parentId: string | null }) =>
      setParent(id, parentId),
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
    },
  });
}
