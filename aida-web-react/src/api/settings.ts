import type {
  RelationshipDefinition,
  CustomTypeDefinition,
  ReactionDefinition,
  IdConfiguration,
} from '@shared/types';
import { apiFetch } from './client';

// --- Metadata ---

export interface StoreMetadata {
  name: string;
  title: string;
  description: string;
}

export function fetchMetadata(): Promise<StoreMetadata> {
  return apiFetch<StoreMetadata>('/v2/settings/metadata');
}

export function updateMetadata(data: Partial<StoreMetadata>): Promise<StoreMetadata> {
  return apiFetch<StoreMetadata>('/v2/settings/metadata', {
    method: 'PUT',
    body: JSON.stringify(data),
  });
}

// --- Relationship definitions ---

export function fetchRelationshipDefs(): Promise<RelationshipDefinition[]> {
  return apiFetch<RelationshipDefinition[]>('/v2/settings/relationship-definitions');
}

export function createRelationshipDef(def: RelationshipDefinition): Promise<RelationshipDefinition> {
  return apiFetch<RelationshipDefinition>('/v2/settings/relationship-definitions', {
    method: 'POST',
    body: JSON.stringify(def),
  });
}

export function updateRelationshipDef(name: string, def: RelationshipDefinition): Promise<RelationshipDefinition> {
  return apiFetch<RelationshipDefinition>(`/v2/settings/relationship-definitions/${encodeURIComponent(name)}`, {
    method: 'PUT',
    body: JSON.stringify(def),
  });
}

export function deleteRelationshipDef(name: string): Promise<void> {
  return apiFetch(`/v2/settings/relationship-definitions/${encodeURIComponent(name)}`, {
    method: 'DELETE',
  });
}

// --- Type definitions ---

export function fetchTypeDefs(): Promise<CustomTypeDefinition[]> {
  return apiFetch<CustomTypeDefinition[]>('/v2/settings/type-definitions');
}

export function createTypeDef(def: CustomTypeDefinition): Promise<CustomTypeDefinition> {
  return apiFetch<CustomTypeDefinition>('/v2/settings/type-definitions', {
    method: 'POST',
    body: JSON.stringify(def),
  });
}

export function updateTypeDef(name: string, def: CustomTypeDefinition): Promise<CustomTypeDefinition> {
  return apiFetch<CustomTypeDefinition>(`/v2/settings/type-definitions/${encodeURIComponent(name)}`, {
    method: 'PUT',
    body: JSON.stringify(def),
  });
}

export function deleteTypeDef(name: string): Promise<void> {
  return apiFetch(`/v2/settings/type-definitions/${encodeURIComponent(name)}`, {
    method: 'DELETE',
  });
}

// --- Reaction definitions ---

export function fetchReactionDefs(): Promise<ReactionDefinition[]> {
  return apiFetch<ReactionDefinition[]>('/v2/settings/reaction-definitions');
}

export function createReactionDef(def: ReactionDefinition): Promise<ReactionDefinition> {
  return apiFetch<ReactionDefinition>('/v2/settings/reaction-definitions', {
    method: 'POST',
    body: JSON.stringify(def),
  });
}

export function updateReactionDef(name: string, def: ReactionDefinition): Promise<ReactionDefinition> {
  return apiFetch<ReactionDefinition>(`/v2/settings/reaction-definitions/${encodeURIComponent(name)}`, {
    method: 'PUT',
    body: JSON.stringify(def),
  });
}

export function deleteReactionDef(name: string): Promise<void> {
  return apiFetch(`/v2/settings/reaction-definitions/${encodeURIComponent(name)}`, {
    method: 'DELETE',
  });
}

// --- ID config ---

export function fetchIdConfig(): Promise<IdConfiguration> {
  return apiFetch<IdConfiguration>('/v2/settings/id-config');
}

export function updateIdConfig(config: IdConfiguration): Promise<IdConfiguration> {
  return apiFetch<IdConfiguration>('/v2/settings/id-config', {
    method: 'PUT',
    body: JSON.stringify(config),
  });
}

// --- Prefixes ---

export interface PrefixConfig {
  allowed_prefixes: string[];
  restrict_prefixes: boolean;
}

export function fetchPrefixes(): Promise<PrefixConfig> {
  return apiFetch<PrefixConfig>('/v2/settings/prefixes');
}

export function updatePrefixes(config: PrefixConfig): Promise<PrefixConfig> {
  return apiFetch<PrefixConfig>('/v2/settings/prefixes', {
    method: 'PUT',
    body: JSON.stringify(config),
  });
}
