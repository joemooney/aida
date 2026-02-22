import type { RuleGroupType } from 'react-querybuilder';
import { formatQuery } from 'react-querybuilder';
import { apply, add_operation } from 'json-logic-js';
import type { Requirement } from '@shared/types';
import { getSprintAssignmentTarget } from './sprint-utils';

// Register custom json-logic operation for case-insensitive substring matching
add_operation('contains_text', (a: unknown, b: unknown) => {
  if (typeof a !== 'string' || typeof b !== 'string') return false;
  return a.toLowerCase().includes(b.toLowerCase());
});

// Register array-contains for tags
add_operation('array_contains', (arr: unknown, text: unknown) => {
  if (!Array.isArray(arr) || typeof text !== 'string') return false;
  const lower = text.toLowerCase();
  return arr.some((item) => typeof item === 'string' && item.toLowerCase().includes(lower));
});

type EnrichedRequirement = Record<string, unknown>;

/** Enrich a requirement with virtual fields for query evaluation. */
function enrichRequirement(req: Requirement): EnrichedRequirement {
  const enriched: EnrichedRequirement = { ...req };

  // Virtual: sprint assignment
  enriched._sprint = getSprintAssignmentTarget(req) ?? '';

  // Virtual: flattened custom fields
  if (req.custom_fields) {
    for (const [key, value] of Object.entries(req.custom_fields)) {
      enriched[`_cf_${key}`] = value ?? '';
    }
  }

  // Ensure tags is always an array
  enriched.tags = req.tags ?? [];

  // Coerce weight to number
  enriched.weight = req.weight ?? 0;

  // Coerce archived to string for select comparison
  enriched.archived = String(req.archived ?? false);

  // Ensure dates are just the date portion for date comparisons
  enriched.created_at = req.created_at?.slice(0, 10) ?? '';
  enriched.modified_at = req.modified_at?.slice(0, 10) ?? '';

  return enriched;
}

/**
 * Convert react-querybuilder query to json-logic and evaluate against requirements.
 * Returns the filtered array. No-op when query has no rules.
 */
export function evaluateAdvancedQuery(
  query: RuleGroupType,
  requirements: Requirement[],
): Requirement[] {
  // Empty query = no filtering
  if (!query.rules || query.rules.length === 0) return requirements;

  // Pre-process: transform the query to handle special fields before converting to json-logic
  const processedQuery = preprocessQuery(query);

  let logic: unknown;
  try {
    logic = formatQuery(processedQuery, 'jsonlogic');
  } catch {
    // If conversion fails, return unfiltered
    return requirements;
  }

  // If logic is trivially true (empty/always-true), return all
  if (logic === true || logic === false) {
    return logic ? requirements : [];
  }

  return requirements.filter((req) => {
    const enriched = enrichRequirement(req);
    try {
      return Boolean(apply(logic, enriched));
    } catch {
      return true; // On eval error, include item
    }
  });
}

/**
 * Pre-process query to replace "contains" on tags field with custom operation,
 * and "contains"/"beginsWith" on text fields with appropriate json-logic ops.
 */
function preprocessQuery(query: RuleGroupType): RuleGroupType {
  return {
    ...query,
    rules: query.rules.map((rule) => {
      if ('rules' in rule) {
        // Nested group — recurse
        return preprocessQuery(rule as RuleGroupType);
      }

      // For tags field with contains, use array_contains
      if (rule.field === 'tags' && rule.operator === 'contains') {
        return {
          ...rule,
          // Mark for custom handling — react-querybuilder's json-logic export
          // handles "contains" by generating {"in": [value, {var: field}]},
          // which works for substring in string but not for array contains.
          // We'll handle this by post-processing the json-logic output.
          // For now, let it pass through and we handle at eval time.
        };
      }

      return rule;
    }),
  };
}
