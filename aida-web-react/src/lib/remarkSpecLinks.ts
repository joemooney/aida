// trace:STORY-0369 | ai:claude
// Remark plugin that auto-links requirement spec IDs (e.g., EPIC-0365, FR-0042)
// to clickable links that open the detail panel.

import { visit } from 'unist-util-visit';
import type { Root, Text, Link, PhrasingContent } from 'mdast';

// Match spec IDs: 2+ uppercase letters, dash, 3-5 digits (e.g., FR-0042, EPIC-0365)
const SPEC_ID_RE = /\b([A-Z]{2,}-\d{3,5})\b/g;

export function remarkSpecLinks() {
  return (tree: Root) => {
    visit(tree, 'text', (node: Text, index, parent) => {
      if (!parent || index === undefined) return;

      // Don't transform text inside existing links or code blocks
      const ptype = parent.type as string;
      if (ptype === 'link' || ptype === 'code' || ptype === 'inlineCode') return;

      const matches = [...node.value.matchAll(SPEC_ID_RE)];
      if (matches.length === 0) return;

      const children: PhrasingContent[] = [];
      let lastIndex = 0;

      for (const match of matches) {
        const specId = match[1];
        const start = match.index!;

        // Text before this match
        if (start > lastIndex) {
          children.push({ type: 'text', value: node.value.slice(lastIndex, start) });
        }

        // Link node for the spec ID
        const link: Link = {
          type: 'link',
          url: `#req:${specId}`,
          children: [{ type: 'text', value: specId }],
        };
        children.push(link);

        lastIndex = start + specId.length;
      }

      // Remaining text after last match
      if (lastIndex < node.value.length) {
        children.push({ type: 'text', value: node.value.slice(lastIndex) });
      }

      // Replace the text node with our mixed content
      parent.children.splice(index, 1, ...children);
    });
  };
}
