import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { fetchSkills, fetchSkill, updateSkill } from '../api/skills';
import type { SkillInfo, SkillDetail } from '../api/skills';

export function useSkills() {
  return useQuery<SkillInfo[]>({
    queryKey: ['skills'],
    queryFn: fetchSkills,
    staleTime: 60_000,
  });
}

export function useSkill(name: string | null) {
  return useQuery<SkillDetail>({
    queryKey: ['skill', name],
    queryFn: () => fetchSkill(name!),
    enabled: !!name,
  });
}

export function useUpdateSkill() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ name, content }: { name: string; content: string }) =>
      updateSkill(name, content),
    onSuccess: (_data, { name }) => {
      queryClient.invalidateQueries({ queryKey: ['skills'] });
      queryClient.invalidateQueries({ queryKey: ['skill', name] });
    },
  });
}
