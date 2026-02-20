import { apiFetch } from './client';

export interface SkillInfo {
  name: string;
  description: string;
  kind: 'skill' | 'command';
}

export interface SkillDetail {
  name: string;
  description: string;
  kind: 'skill' | 'command';
  content: string;
  allowed_tools: string[];
}

export function fetchSkills(): Promise<SkillInfo[]> {
  return apiFetch<SkillInfo[]>('/v2/skills');
}

export function fetchSkill(name: string): Promise<SkillDetail> {
  return apiFetch<SkillDetail>(`/v2/skills/${encodeURIComponent(name)}`);
}

export function updateSkill(name: string, content: string): Promise<SkillDetail> {
  return apiFetch<SkillDetail>(`/v2/skills/${encodeURIComponent(name)}`, {
    method: 'PUT',
    body: JSON.stringify({ content }),
  });
}
