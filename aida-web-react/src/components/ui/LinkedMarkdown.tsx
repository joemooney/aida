// trace:STORY-0369 | ai:claude
// Markdown renderer that auto-links spec IDs (e.g., EPIC-0365) to the detail panel.

import { useCallback, type ComponentPropsWithoutRef } from 'react';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { oneDark } from 'react-syntax-highlighter/dist/esm/styles/prism';
import { remarkSpecLinks } from '../../lib/remarkSpecLinks';
import { remarkColorText, COLOR_CLASSES } from '../../lib/remarkColorText';
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
      // Colored text: ::red[text] → <span class="text-red-400">text</span>
      if (href?.startsWith('#color:')) {
        const colorName = href.slice(7);
        const colorClass = COLOR_CLASSES[colorName] ?? '';
        return <span className={colorClass}>{linkChildren}</span>;
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

  const CodeComponent = useCallback(
    ({ className: codeClassName, children: codeChildren, ...props }: ComponentPropsWithoutRef<'code'>) => {
      const match = /language-(\w+)/.exec(codeClassName || '');
      const codeString = String(codeChildren).replace(/\n$/, '');
      // Fenced code block with language
      if (match) {
        return (
          <SyntaxHighlighter
            style={oneDark}
            language={match[1]}
            PreTag="div"
            customStyle={{ margin: 0, borderRadius: '0.5rem', fontSize: '0.75rem' }}
          >
            {codeString}
          </SyntaxHighlighter>
        );
      }
      // Fenced code block without language (detect by presence of newlines)
      if (codeString.includes('\n')) {
        return (
          <SyntaxHighlighter
            style={oneDark}
            language="text"
            PreTag="div"
            customStyle={{ margin: 0, borderRadius: '0.5rem', fontSize: '0.75rem' }}
          >
            {codeString}
          </SyntaxHighlighter>
        );
      }
      // Inline code
      return <code className={codeClassName} {...props}>{codeChildren}</code>;
    },
    [],
  );

  return (
    <div className={className}>
      <Markdown
        remarkPlugins={[remarkGfm, remarkSpecLinks, remarkColorText]}
        components={{ a: AnchorComponent, code: CodeComponent }}
      >
        {children}
      </Markdown>
    </div>
  );
}
