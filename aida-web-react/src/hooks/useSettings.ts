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
import { requireWrite, usePermissions } from './usePermissions';

// --- Metadata ---

export function useMetadata() {
  return useQuery<StoreMetadata>({
    queryKey: ['settings', 'metadata'],
    queryFn: fetchMetadata,
    staleTime: 60_000,
  });
}

export function useUpdateMetadata() {
  const { canWrite } = usePermissions();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (data: Partial<StoreMetadata>) => {
      requireWrite(canWrite);
      return updateMetadata(data);
    },
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
  const { canWrite } = usePermissions();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (def: RelationshipDefinition) => {
      requireWrite(canWrite);
      return createRelationshipDef(def);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'relationship-defs'] }),
  });
}

export function useUpdateRelDef() {
  const { canWrite } = usePermissions();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ name, def }: { name: string; def: RelationshipDefinition }) => {
      requireWrite(canWrite);
      return updateRelationshipDef(name, def);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'relationship-defs'] }),
  });
}

export function useDeleteRelDef() {
  const { canWrite } = usePermissions();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => {
      requireWrite(canWrite);
      return deleteRelationshipDef(name);
    },
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
  const { canWrite } = usePermissions();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (def: CustomTypeDefinition) => {
      requireWrite(canWrite);
      return createTypeDef(def);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'type-defs'] }),
  });
}

export function useUpdateTypeDef() {
  const { canWrite } = usePermissions();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ name, def }: { name: string; def: CustomTypeDefinition }) => {
      requireWrite(canWrite);
      return updateTypeDef(name, def);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'type-defs'] }),
  });
}

export function useDeleteTypeDef() {
  const { canWrite } = usePermissions();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => {
      requireWrite(canWrite);
      return deleteTypeDef(name);
    },
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
  const { canWrite } = usePermissions();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (def: ReactionDefinition) => {
      requireWrite(canWrite);
      return createReactionDef(def);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'reaction-defs'] }),
  });
}

export function useUpdateReactionDef() {
  const { canWrite } = usePermissions();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ name, def }: { name: string; def: ReactionDefinition }) => {
      requireWrite(canWrite);
      return updateReactionDef(name, def);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'reaction-defs'] }),
  });
}

export function useDeleteReactionDef() {
  const { canWrite } = usePermissions();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => {
      requireWrite(canWrite);
      return deleteReactionDef(name);
    },
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
  const { canWrite } = usePermissions();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (config: IdConfiguration) => {
      requireWrite(canWrite);
      return updateIdConfig(config);
    },
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
  const { canWrite } = usePermissions();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (config: PrefixConfig) => {
      requireWrite(canWrite);
      return updatePrefixes(config);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ['settings', 'prefixes'] }),
  });
}
