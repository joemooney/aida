// trace:STORY-0369 | ai:claude
// Markdown renderer that auto-links spec IDs (e.g., EPIC-0365) to the detail panel.

import { useCallback, type ComponentPropsWithoutRef } from 'react';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { remarkSpecLinks } from '../../lib/remarkSpecLinks';
import { useDetailPanel } from '../../hooks/useDetailPanel';

interface LinkedMarkdownProps {
  children: string | null | undefined;
  className?: string;
}

export function LinkedMarkdown({ children, className }: LinkedMarkdownProps) {
  const { open } = useDetailPanel();

  const AnchorComponent = useCallback(
    ({ href, children: linkChildren, ...props }: ComponentPropsWithoutRef<'a'>) => {
      if (href?.startsWith('#req:')) {
        const specId = href.slice(5);
        return (
          <a
            {...props}
            href={href}
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              open(specId);
            }}
            title={`Open ${specId}`}
          >
            {linkChildren}
          </a>
        );
      }
      // Normal external links
      return (
        <a href={href} {...props} target="_blank" rel="noopener noreferrer">
          {linkChildren}
        </a>
      );
    },
    [open],
  );

  return (
    <div className={className}>
      <Markdown
        remarkPlugins={[remarkGfm, remarkSpecLinks]}
        components={{ a: AnchorComponent }}
      >
        {children}
      </Markdown>
    </div>
  );
}
