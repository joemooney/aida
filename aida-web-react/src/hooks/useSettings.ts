import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import type {
  RelationshipDefinition,
  CustomTypeDefinition,
  ReactionDefinition,
  IdConfiguration,
} from '@shared/types';
import {
  fetchMetadata, updateMetadata,
  fetchRelationshipDefs, createRelationshipDef, updateRelationshipDef, deleteRelationshipDef,
  fetchTypeDefs, createTypeDef, updateTypeDef, deleteTypeDef,
  fetchReactionDefs, createReactionDef, updateReactionDef, deleteReactionDef,
  fetchIdConfig, updateIdConfig,
  fetchPrefixes, updatePrefixes,
  type StoreMetadata, type PrefixConfig,
} from '../api/settings';

// --- Metadata ---

export function useMetadata() {
  return useQuery<StoreMetadata>({
    queryKey: ['settings', 'metadata'],
    queryFn: fetchMetadata,
    staleTime: 60_000,
  });
}

export function useUpdateMetadata() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (data: Partial<StoreMetadata>) => updateMetadata(data),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'metadata'] }),
  });
}

// --- Relationship definitions ---

export function useRelationshipDefs() {
  return useQuery<RelationshipDefinition[]>({
    queryKey: ['settings', 'relationship-defs'],
    queryFn: fetchRelationshipDefs,
    staleTime: 60_000,
  });
}

export function useCreateRelDef() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (def: RelationshipDefinition) => createRelationshipDef(def),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'relationship-defs'] }),
  });
}

export function useUpdateRelDef() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ name, def }: { name: string; def: RelationshipDefinition }) =>
      updateRelationshipDef(name, def),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'relationship-defs'] }),
  });
}

export function useDeleteRelDef() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => deleteRelationshipDef(name),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'relationship-defs'] }),
  });
}

// --- Type definitions ---

export function useTypeDefs() {
  return useQuery<CustomTypeDefinition[]>({
    queryKey: ['settings', 'type-defs'],
    queryFn: fetchTypeDefs,
    staleTime: 60_000,
  });
}

export function useCreateTypeDef() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (def: CustomTypeDefinition) => createTypeDef(def),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'type-defs'] }),
  });
}

export function useUpdateTypeDef() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ name, def }: { name: string; def: CustomTypeDefinition }) =>
      updateTypeDef(name, def),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'type-defs'] }),
  });
}

export function useDeleteTypeDef() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => deleteTypeDef(name),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'type-defs'] }),
  });
}

// --- Reaction definitions ---

export function useReactionDefs() {
  return useQuery<ReactionDefinition[]>({
    queryKey: ['settings', 'reaction-defs'],
    queryFn: fetchReactionDefs,
    staleTime: 60_000,
  });
}

export function useCreateReactionDef() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (def: ReactionDefinition) => createReactionDef(def),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'reaction-defs'] }),
  });
}

export function useUpdateReactionDef() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ name, def }: { name: string; def: ReactionDefinition }) =>
      updateReactionDef(name, def),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'reaction-defs'] }),
  });
}

export function useDeleteReactionDef() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => deleteReactionDef(name),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'reaction-defs'] }),
  });
}

// --- ID config ---

export function useIdConfig() {
  return useQuery<IdConfiguration>({
    queryKey: ['settings', 'id-config'],
    queryFn: fetchIdConfig,
    staleTime: 60_000,
  });
}

export function useUpdateIdConfig() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (config: IdConfiguration) => updateIdConfig(config),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'id-config'] }),
  });
}

// --- Prefixes ---

export function usePrefixes() {
  return useQuery<PrefixConfig>({
    queryKey: ['settings', 'prefixes'],
    queryFn: fetchPrefixes,
    staleTime: 60_000,
  });
}

export function useUpdatePrefixes() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (config: PrefixConfig) => updatePrefixes(config),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'prefixes'] }),
  });
}
