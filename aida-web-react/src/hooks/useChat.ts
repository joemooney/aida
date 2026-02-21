// trace:STORY-0374 | ai:claude
import { useState, useCallback, useRef } from 'react';
import { useQuery } from '@tanstack/react-query';
import { fetchChatStatus, sendChatMessage, type ChatMessage } from '../api/chat';

export interface DisplayMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  timestamp: number;
}

export function useChatStatus() {
  return useQuery({
    queryKey: ['chat-status'],
    queryFn: fetchChatStatus,
    staleTime: 60_000,
  });
}

export function useChat() {
  const [messages, setMessages] = useState<DisplayMessage[]>([]);
  const [isStreaming, setIsStreaming] = useState(false);
  const abortRef = useRef<AbortController | null>(null);

  const send = useCallback(async (text: string) => {
    if (!text.trim() || isStreaming) return;

    // Add user message
    const userMsg: DisplayMessage = {
      id: crypto.randomUUID(),
      role: 'user',
      content: text.trim(),
      timestamp: Date.now(),
    };

    // Create placeholder assistant message
    const assistantMsg: DisplayMessage = {
      id: crypto.randomUUID(),
      role: 'assistant',
      content: '',
      timestamp: Date.now(),
    };

    setMessages((prev) => [...prev, userMsg, assistantMsg]);
    setIsStreaming(true);

    // Build full conversation history for the API
    const history: ChatMessage[] = [
      ...messages.map((m) => ({ role: m.role, content: m.content })),
      { role: 'user' as const, content: text.trim() },
    ];

    try {
      const response = await sendChatMessage(history);
      const reader = response.body?.getReader();
      if (!reader) throw new Error('No response body');

      const decoder = new TextDecoder();
      let buffer = '';

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        buffer += decoder.decode(value, { stream: true });

        // Parse SSE events from buffer
        while (true) {
          const eventEnd = buffer.indexOf('\n\n');
          if (eventEnd === -1) break;

          const eventStr = buffer.slice(0, eventEnd);
          buffer = buffer.slice(eventEnd + 2);

          let eventType = '';
          let data = '';

          for (const line of eventStr.split('\n')) {
            if (line.startsWith('event: ')) {
              eventType = line.slice(7);
            } else if (line.startsWith('data: ')) {
              data = line.slice(6);
            }
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
              // skip malformed JSON
            }
          } else if (eventType === 'done') {
            break;
          } else if (eventType === 'error') {
            setMessages((prev) => {
              const updated = [...prev];
              const last = updated[updated.length - 1];
              if (last && last.id === assistantMsg.id) {
                updated[updated.length - 1] = {
                  ...last,
                  content: 'Sorry, an error occurred. Please try again.',
                };
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
  }, [messages, isStreaming]);

  const clear = useCallback(() => {
    if (abortRef.current) {
      abortRef.current.abort();
      abortRef.current = null;
    }
    setMessages([]);
    setIsStreaming(false);
  }, []);

  return { messages, isStreaming, send, clear };
}
