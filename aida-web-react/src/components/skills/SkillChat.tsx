// trace:TASK-0001 | ai:claude
import { useState, useCallback, useRef, useEffect, type KeyboardEvent } from 'react';
import { MessageCircle, Send, ChevronDown, ChevronRight } from 'lucide-react';
import { cn } from '../../lib/utils';
import { sendSkillChat, type SkillChatMessage } from '../../api/skillRunner';
import type { WarningsReport } from '../../api/skillRunner';
import { LinkedMarkdown } from '../ui/LinkedMarkdown';
import { Spinner } from '../ui/Spinner';

interface SkillChatProps {
  skillName: string;
  warningsReport: WarningsReport;
}

interface DisplayMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  timestamp: number;
}

const STARTER_QUESTIONS = [
  'Which dead_code warnings are safe to remove?',
  'Are any warnings potential bugs?',
  'Prioritize the warnings by importance',
  'What should I fix first?',
];

export function SkillChat({ skillName, warningsReport }: SkillChatProps) {
  const [expanded, setExpanded] = useState(false);
  const [messages, setMessages] = useState<DisplayMessage[]>([]);
  const [isStreaming, setIsStreaming] = useState(false);
  const [input, setInput] = useState('');
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Auto-scroll
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  // Auto-resize textarea
  useEffect(() => {
    const ta = textareaRef.current;
    if (ta) {
      ta.style.height = 'auto';
      ta.style.height = `${Math.min(ta.scrollHeight, 120)}px`;
    }
  }, [input]);

  const send = useCallback(async (text: string) => {
    if (!text.trim() || isStreaming) return;

    const userMsg: DisplayMessage = {
      id: crypto.randomUUID(),
      role: 'user',
      content: text.trim(),
      timestamp: Date.now(),
    };

    const assistantMsg: DisplayMessage = {
      id: crypto.randomUUID(),
      role: 'assistant',
      content: '',
      timestamp: Date.now(),
    };

    setMessages((prev) => [...prev, userMsg, assistantMsg]);
    setIsStreaming(true);
    setInput('');

    const history: SkillChatMessage[] = [
      ...messages.map((m) => ({ role: m.role, content: m.content })),
      { role: 'user' as const, content: text.trim() },
    ];

    try {
      const response = await sendSkillChat(skillName, history, warningsReport);
      const reader = response.body?.getReader();
      if (!reader) throw new Error('No response body');

      const decoder = new TextDecoder();
      let buffer = '';

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        buffer += decoder.decode(value, { stream: true });

        while (true) {
          const eventEnd = buffer.indexOf('\n\n');
          if (eventEnd === -1) break;

          const eventStr = buffer.slice(0, eventEnd);
          buffer = buffer.slice(eventEnd + 2);

          let eventType = '';
          let data = '';

          for (const line of eventStr.split('\n')) {
            if (line.startsWith('event: ')) eventType = line.slice(7);
            else if (line.startsWith('data: ')) data = line.slice(6);
          }

          if (eventType === 'delta') {
            try {
              const parsed = JSON.parse(data);
              if (parsed.text) {
                setMessages((prev) => {
                  const updated = [...prev];
                  const last = updated[updated.length - 1];
                  if (last && last.id === assistantMsg.id) {
                    updated[updated.length - 1] = {
                      ...last,
                      content: last.content + parsed.text,
                    };
                  }
                  return updated;
                });
              }
            } catch {
              // skip malformed
            }
          } else if (eventType === 'done') {
            break;
          } else if (eventType === 'error') {
            let errorText = 'Sorry, an error occurred.';
            try {
              const parsed = JSON.parse(data);
              if (parsed.error) errorText = `Error: ${parsed.error}`;
            } catch {
              if (data) errorText = `Error: ${data}`;
            }
            setMessages((prev) => {
              const updated = [...prev];
              const last = updated[updated.length - 1];
              if (last && last.id === assistantMsg.id) {
                updated[updated.length - 1] = { ...last, content: errorText };
              }
              return updated;
            });
            break;
          }
        }
      }
    } catch (err) {
      setMessages((prev) => {
        const updated = [...prev];
        const last = updated[updated.length - 1];
        if (last && last.id === assistantMsg.id) {
          updated[updated.length - 1] = {
            ...last,
            content: `Error: ${err instanceof Error ? err.message : 'Unknown error'}`,
          };
        }
        return updated;
      });
    } finally {
      setIsStreaming(false);
    }
  }, [messages, isStreaming, skillName, warningsReport]);

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      send(input);
    }
  };

  return (
    <div className="border-t border-edge pt-4 mt-4">
      <button
        onClick={() => setExpanded((e) => !e)}
        className="flex items-center gap-2 cursor-pointer mb-3"
      >
        {expanded ? (
          <ChevronDown className="h-3.5 w-3.5 text-content-muted" />
        ) : (
          <ChevronRight className="h-3.5 w-3.5 text-content-muted" />
        )}
        <MessageCircle className="h-4 w-4 text-accent" />
        <span className="text-sm font-medium text-content">Ask about these warnings</span>
      </button>

      {expanded && (
        <div className="space-y-3">
          {/* Starter questions */}
          {messages.length === 0 && (
            <div className="flex flex-wrap gap-2">
              {STARTER_QUESTIONS.map((q) => (
                <button
                  key={q}
                  onClick={() => send(q)}
                  disabled={isStreaming}
                  className="rounded-lg border border-edge bg-surface-hover px-3 py-1.5 text-xs text-content-secondary hover:text-content hover:border-accent/40 transition-colors cursor-pointer disabled:opacity-50"
                >
                  {q}
                </button>
              ))}
            </div>
          )}

          {/* Messages */}
          {messages.length > 0 && (
            <div className="space-y-3 max-h-80 overflow-y-auto">
              {messages.map((msg) => (
                <div
                  key={msg.id}
                  className={cn(
                    'text-xs',
                    msg.role === 'user' ? 'text-content' : 'text-content-secondary',
                  )}
                >
                  <span className="font-medium text-[11px] uppercase tracking-wider text-content-muted">
                    {msg.role === 'user' ? 'You' : 'AIDA'}
                  </span>
                  {msg.role === 'assistant' ? (
                    msg.content ? (
                      <LinkedMarkdown className="mt-1 prose prose-sm prose-invert max-w-none text-content-secondary prose-headings:text-content prose-strong:text-content prose-code:text-accent prose-code:bg-surface-hover prose-code:px-1 prose-code:py-0.5 prose-code:rounded prose-code:text-xs prose-code:before:content-none prose-code:after:content-none">
                        {msg.content}
                      </LinkedMarkdown>
                    ) : (
                      <div className="mt-1 flex items-center gap-2">
                        <Spinner size="sm" />
                        <span className="text-content-muted">Thinking...</span>
                      </div>
                    )
                  ) : (
                    <p className="mt-1">{msg.content}</p>
                  )}
                </div>
              ))}
              <div ref={messagesEndRef} />
            </div>
          )}

          {/* Input */}
          <div className="flex gap-2">
            <textarea
              ref={textareaRef}
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={handleKeyDown}
              placeholder="Ask about the warnings..."
              rows={1}
              className="flex-1 rounded-lg border border-edge bg-surface px-3 py-2 text-xs text-content placeholder:text-content-muted resize-none focus:outline-none focus:border-accent/50"
            />
            <button
              onClick={() => send(input)}
              disabled={!input.trim() || isStreaming}
              className="rounded-lg bg-accent px-3 py-2 text-white hover:bg-accent-hover transition-colors cursor-pointer disabled:opacity-50 shrink-0"
            >
              <Send className="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
