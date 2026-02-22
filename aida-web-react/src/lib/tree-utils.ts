// trace:TASK-014 | ai:claude
import type { Requirement } from '@shared/types';

export interface TreeNode {
  requirement: Requirement;
  children: TreeNode[];
  depth: number;
}

export interface FlatTreeRow {
  node: TreeNode;
  isAncestorOnly: boolean;
}

/**
 * Check if a relationship is a Parent relationship.
 * A requirement with rel_type "Parent" points to its parent via target_id.
 */
function isParentRelationship(rel: { rel_type: unknown }): boolean {
  return rel.rel_type === 'Parent';
}

/**
 * Build a tree from a flat list of requirements using Parent relationships.
 * Each requirement's relationships array may contain { rel_type: "Parent", target_id: "<parent>" }.
 * When filteredIds is provided, ancestor nodes are included for context.
 */
export function buildTree(
  allRequirements: Requirement[],
  filteredIds?: Set<string>,
): { roots: TreeNode[]; ancestorIds: Set<string> } {
  // Index requirements by both id and spec_id for lookup
  const byId = new Map<string, Requirement>();
  const bySpecId = new Map<string, Requirement>();
  for (const req of allRequirements) {
    byId.set(req.id, req);
    if (req.spec_id) bySpecId.set(req.spec_id, req);
  }

  // Build child -> parent mapping
  const childToParent = new Map<string, string>(); // child id -> parent id
  for (const req of allRequirements) {
    const parentRel = req.relationships?.find(isParentRelationship);
    if (parentRel) {
      // target_id could be a UUID or a spec_id
      const parentReq = byId.get(parentRel.target_id) ?? bySpecId.get(parentRel.target_id);
      if (parentReq) {
        childToParent.set(req.id, parentReq.id);
      }
    }
  }

  // Build parent -> children mapping
  const parentToChildren = new Map<string, Requirement[]>();
  for (const [childId, parentId] of childToParent) {
    const child = byId.get(childId)!;
    const existing = parentToChildren.get(parentId) ?? [];
    existing.push(child);
    parentToChildren.set(parentId, existing);
  }

  // Compute ancestor IDs for filtered context
  const ancestorIds = new Set<string>();
  if (filteredIds) {
    for (const id of filteredIds) {
      let current = childToParent.get(id);
      while (current) {
        if (ancestorIds.has(current)) break;
        ancestorIds.add(current);
        current = childToParent.get(current);
      }
    }
  }

  // Sort children by spec_id at each level
  function sortBySpecId(reqs: Requirement[]): Requirement[] {
    return [...reqs].sort((a, b) => {
      const aId = a.spec_id ?? '';
      const bId = b.spec_id ?? '';
      return aId.localeCompare(bId, undefined, { numeric: true });
    });
  }

  // Recursively build tree nodes
  function buildNodes(reqs: Requirement[], depth: number): TreeNode[] {
    return sortBySpecId(reqs).map((req) => {
      const children = parentToChildren.get(req.id) ?? [];
      return {
        requirement: req,
        children: buildNodes(children, depth + 1),
        depth,
      };
    });
  }

  // Roots = requirements that have no parent
  const roots = allRequirements.filter((req) => !childToParent.has(req.id));
  return { roots: buildNodes(roots, 0), ancestorIds };
}

/**
 * Flatten a tree into a list of visible rows, respecting collapsed state.
 * When filteredIds is provided, only filtered items and their ancestors are included.
 */
export function flattenTree(
  roots: TreeNode[],
  collapsedSet: Set<string>,
  filteredIds?: Set<string>,
  ancestorIds?: Set<string>,
): FlatTreeRow[] {
  const rows: FlatTreeRow[] = [];

  function walk(nodes: TreeNode[]) {
    for (const node of nodes) {
      const id = node.requirement.id;

      // When filtering, skip items that are neither filtered nor ancestors
      if (filteredIds && !filteredIds.has(id) && !ancestorIds?.has(id)) {
        continue;
      }

      const isAncestorOnly = filteredIds ? !filteredIds.has(id) && (ancestorIds?.has(id) ?? false) : false;
      rows.push({ node, isAncestorOnly });

      // If not collapsed, recurse into children
      if (!collapsedSet.has(id) && node.children.length > 0) {
        walk(node.children);
      }
    }
  }

  walk(roots);
  return rows;
}

/**
 * Check if candidateId is a descendant of ancestorId in the tree.
 * Used to prevent circular references when reparenting.
 */
export function isDescendant(roots: TreeNode[], ancestorId: string, candidateId: string): boolean {
  function search(nodes: TreeNode[]): boolean {
    for (const node of nodes) {
      if (node.requirement.id === ancestorId) {
        return findInChildren(node.children);
      }
      if (search(node.children)) return true;
    }
    return false;
  }
  function findInChildren(nodes: TreeNode[]): boolean {
    for (const node of nodes) {
      if (node.requirement.id === candidateId) return true;
      if (findInChildren(node.children)) return true;
    }
    return false;
  }
  return search(roots);
}

/**
 * Collect all node IDs that have children (for expand/collapse all).
 */
export function collectParentIds(roots: TreeNode[]): Set<string> {
  const ids = new Set<string>();
  function walk(nodes: TreeNode[]) {
    for (const node of nodes) {
      if (node.children.length > 0) {
        ids.add(node.requirement.id);
        walk(node.children);
      }
    }
  }
  walk(roots);
  return ids;
}
