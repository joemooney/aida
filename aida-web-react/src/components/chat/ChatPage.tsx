// trace:STORY-0374 | ai:claude
import { useState, useRef, useEffect, type KeyboardEvent } from 'react';
import { MessageCircle, Send, Trash2, Bot, User } from 'lucide-react';
import { cn } from '../../lib/utils';
import { useChat, useChatStatus, type DisplayMessage } from '../../hooks/useChat';
import { LinkedMarkdown } from '../ui/LinkedMarkdown';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';

const STARTER_QUESTIONS = [
  'What is the overall project status?',
  'Which items are high priority and still in draft?',
  'What was completed in the latest sprint?',
  'Are there any blocked or at-risk items?',
  'Summarize progress by feature area',
];

export function ChatPage() {
  const { data: status, isLoading: statusLoading } = useChatStatus();
  const { messages, isStreaming, send, clear } = useChat();
  const [input, setInput] = useState('');
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Auto-scroll on new content
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  // Auto-resize textarea
  useEffect(() => {
    const ta = textareaRef.current;
    if (ta) {
      ta.style.height = 'auto';
      ta.style.height = `${Math.min(ta.scrollHeight, 160)}px`;
    }
  }, [input]);

  const handleSend = () => {
    if (!input.trim() || isStreaming) return;
    send(input);
    setInput('');
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const handleStarter = (question: string) => {
    send(question);
  };

  // Unavailable state
  if (statusLoading) {
    return (
      <div className="flex h-full items-center justify-center">
        <Spinner size="lg" />
      </div>
    );
  }

  if (status && !status.available) {
    return (
      <EmptyState
        icon={<MessageCircle className="h-12 w-12" />}
        title="Chat is not available"
        description={status.reason || 'The ANTHROPIC_API_KEY environment variable is not set on the server.'}
        className="h-full"
      />
    );
  }

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-edge px-6 h-14 shrink-0">
        <div className="flex items-center gap-2">
          <MessageCircle className="h-5 w-5 text-accent" />
          <h1 className="text-lg font-semibold text-content">AIDA Chat</h1>
          {status?.reason && (
            <span className="text-xs text-content-muted ml-2">({status.reason})</span>
          )}
        </div>
        {messages.length > 0 && (
          <button
            onClick={clear}
            className="flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-sm text-content-secondary hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
          >
            <Trash2 className="h-4 w-4" />
            Clear
          </button>
        )}
      </div>

      {/* Messages area */}
      <div className="flex-1 overflow-y-auto px-6 py-4">
        {messages.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full gap-6">
            <div className="text-center">
              <Bot className="h-12 w-12 text-accent mx-auto mb-3" />
              <h2 className="text-lg font-semibold text-content">Ask about your project</h2>
              <p className="text-sm text-content-secondary mt-1 max-w-md">
                I have access to all your requirements. Ask me about status, priorities, progress, or anything else.
              </p>
            </div>
            <div className="flex flex-wrap justify-center gap-2 max-w-lg">
              {STARTER_QUESTIONS.map((q) => (
                <button
                  key={q}
                  onClick={() => handleStarter(q)}
                  className="rounded-full border border-edge px-3 py-1.5 text-sm text-content-secondary hover:text-content hover:border-accent hover:bg-accent/5 transition-colors cursor-pointer"
                >
                  {q}
                </button>
              ))}
            </div>
          </div>
        ) : (
          <div className="max-w-3xl mx-auto space-y-4">
            {messages.map((msg) => (
              <MessageBubble key={msg.id} message={msg} isStreaming={isStreaming && msg === messages[messages.length - 1] && msg.role === 'assistant'} />
            ))}
            <div ref={messagesEndRef} />
          </div>
        )}
      </div>

      {/* Input bar */}
      <div className="shrink-0 border-t border-edge px-6 py-3">
        <div className="max-w-3xl mx-auto flex items-end gap-3">
          <textarea
            ref={textareaRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Ask about requirements, status, priorities..."
            disabled={isStreaming}
            rows={1}
            className={cn(
              'flex-1 resize-none rounded-lg border border-edge bg-surface px-4 py-2.5 text-sm text-content placeholder:text-content-muted',
              'focus:outline-none focus:ring-2 focus:ring-accent/40 focus:border-accent',
              'disabled:opacity-50 disabled:cursor-not-allowed',
            )}
          />
          <button
            onClick={handleSend}
            disabled={!input.trim() || isStreaming}
            className={cn(
              'flex h-10 w-10 shrink-0 items-center justify-center rounded-lg transition-colors cursor-pointer',
              input.trim() && !isStreaming
                ? 'bg-accent text-white hover:bg-accent/90'
                : 'bg-surface-hover text-content-muted cursor-not-allowed',
            )}
          >
            {isStreaming ? <Spinner size="sm" /> : <Send className="h-4 w-4" />}
          </button>
        </div>
      </div>
    </div>
  );
}

function MessageBubble({ message, isStreaming }: { message: DisplayMessage; isStreaming: boolean }) {
  const isUser = message.role === 'user';

  return (
    <div className={cn('flex gap-3', isUser && 'flex-row-reverse')}>
      <div className={cn(
        'flex h-8 w-8 shrink-0 items-center justify-center rounded-full',
        isUser ? 'bg-accent text-white' : 'bg-surface-hover text-content-secondary',
      )}>
        {isUser ? <User className="h-4 w-4" /> : <Bot className="h-4 w-4" />}
      </div>
      <div className={cn(
        'rounded-xl px-4 py-2.5 max-w-[80%] text-sm',
        isUser
          ? 'bg-accent text-white rounded-br-sm'
          : 'bg-surface-alt border border-edge rounded-bl-sm',
      )}>
        {isUser ? (
          <p className="whitespace-pre-wrap">{message.content}</p>
        ) : (
          <>
            {message.content ? (
              <LinkedMarkdown className="prose prose-sm dark:prose-invert max-w-none [&>*:first-child]:mt-0 [&>*:last-child]:mb-0">
                {message.content}
              </LinkedMarkdown>
            ) : isStreaming ? (
              <span className="inline-block h-4 w-1.5 bg-content-muted animate-pulse rounded-full" />
            ) : null}
            {isStreaming && message.content && (
              <span className="inline-block h-3.5 w-1 bg-content-muted animate-pulse rounded-full ml-0.5 align-baseline" />
            )}
          </>
        )}
      </div>
    </div>
  );
}
