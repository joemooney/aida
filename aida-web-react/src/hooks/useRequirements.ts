import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import type { Requirement } from '@shared/types';
import { fetchRequirements, fetchRequirement, updateRequirement, createRequirement, setParent } from '../api/requirements';
import { usePermissions, requireWrite } from './usePermissions';

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
  const { canWrite } = usePermissions();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ id, data }: { id: string; data: Partial<Requirement> }) => {
      requireWrite(canWrite);
      return updateRequirement(id, data);
    },
    onMutate: async ({ id, data }) => {
      await queryClient.cancelQueries({ queryKey: ['requirements'] });
      await queryClient.cancelQueries({ queryKey: ['requirement', id] });
      const previous = queryClient.getQueryData<Requirement[]>(['requirements']);
      const previousSingle = queryClient.getQueryData<Requirement>(['requirement', id]);

      queryClient.setQueryData<Requirement[]>(['requirements'], (old) =>
        old?.map((req) =>
          req.id === id || req.spec_id === id ? { ...req, ...data } : req,
        ),
      );
      if (previousSingle) {
        queryClient.setQueryData<Requirement>(['requirement', id], { ...previousSingle, ...data });
      }

      return { previous, previousSingle };
    },
    onError: (_err, { id }, context) => {
      if (context?.previous) {
        queryClient.setQueryData(['requirements'], context.previous);
      }
      if (context?.previousSingle) {
        queryClient.setQueryData(['requirement', id], context.previousSingle);
      }
    },
    onSettled: (_data, _err, { id }) => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
      queryClient.invalidateQueries({ queryKey: ['requirement', id] });
    },
  });
}

export function useCreateRequirement() {
  const { canWrite } = usePermissions();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (data: Partial<Requirement>) => {
      requireWrite(canWrite);
      return createRequirement(data);
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
    },
  });
}

export function useSetParent() {
  const { canWrite } = usePermissions();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, parentId }: { id: string; parentId: string | null }) => {
      requireWrite(canWrite);
      return setParent(id, parentId);
    },
    onSettled: (_data, _err, { id }) => {
      queryClient.invalidateQueries({ queryKey: ['requirements'] });
      queryClient.invalidateQueries({ queryKey: ['requirement', id] });
    },
  });
}
