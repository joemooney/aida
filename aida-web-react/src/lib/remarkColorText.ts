// Remark plugin that converts ::color[text] syntax to colored spans.
// Syntax: ::red[some text], ::blue[other text], etc.
// Outputs link nodes with href="#color:<name>" for react-markdown component mapping.

import { visit } from 'unist-util-visit';
import type { Root, Text, Link, PhrasingContent } from 'mdast';

const COLORS = new Set([
  'red', 'green', 'blue', 'yellow', 'orange', 'purple', 'pink', 'cyan', 'gray', 'grey',
  'amber', 'lime', 'teal', 'indigo', 'violet', 'rose', 'emerald', 'sky', 'slate', 'white',
]);

// Match ::color[text] — color name must be a known color
const COLOR_RE = /::(\w+)\[([^\]]+)\]/g;

export function remarkColorText() {
  return (tree: Root) => {
    visit(tree, 'text', (node: Text, index, parent) => {
      if (!parent || index === undefined) return;
      const ptype = parent.type as string;
      if (ptype === 'link' || ptype === 'code' || ptype === 'inlineCode') return;

      const matches = [...node.value.matchAll(COLOR_RE)];
      if (matches.length === 0) return;

      const children: PhrasingContent[] = [];
      let lastIndex = 0;

      for (const match of matches) {
        const colorName = match[1].toLowerCase();
        const text = match[2];
        const start = match.index!;

        if (!COLORS.has(colorName)) continue;

        if (start > lastIndex) {
          children.push({ type: 'text', value: node.value.slice(lastIndex, start) });
        }

        const link: Link = {
          type: 'link',
          url: `#color:${colorName}`,
          children: [{ type: 'text', value: text }],
        };
        children.push(link);

        lastIndex = start + match[0].length;
      }

      if (children.length === 0) return;

      if (lastIndex < node.value.length) {
        children.push({ type: 'text', value: node.value.slice(lastIndex) });
      }

      parent.children.splice(index, 1, ...children);
    });
  };
}

// Tailwind color classes for the component renderer
export const COLOR_CLASSES: Record<string, string> = {
  red: 'text-red-400',
  green: 'text-green-400',
  blue: 'text-blue-400',
  yellow: 'text-yellow-400',
  orange: 'text-orange-400',
  purple: 'text-purple-400',
  pink: 'text-pink-400',
  cyan: 'text-cyan-400',
  gray: 'text-gray-400',
  grey: 'text-gray-400',
  amber: 'text-amber-400',
  lime: 'text-lime-400',
  teal: 'text-teal-400',
  indigo: 'text-indigo-400',
  violet: 'text-violet-400',
  rose: 'text-rose-400',
  emerald: 'text-emerald-400',
  sky: 'text-sky-400',
  slate: 'text-slate-400',
  white: 'text-white',
};
