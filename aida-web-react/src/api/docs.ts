import { apiFetch } from './client';

export interface DocInfo {
  name: string;
  title: string;
  path: string;
  section: 'docs' | 'plans';
}

export interface DocDetail {
  name: string;
  title: string;
  path: string;
  section: 'docs' | 'plans';
  content: string;
}

export function fetchDocs(): Promise<DocInfo[]> {
  return apiFetch<DocInfo[]>('/v2/docs');
}

export function fetchDoc(path: string): Promise<DocDetail> {
  return apiFetch<DocDetail>(`/v2/docs/${path}`);
}
