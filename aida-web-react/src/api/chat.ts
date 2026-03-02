// trace:STORY-0374 | ai:claude
import { apiFetch, buildApiHeaders } from './client';

export interface ChatStatusResponse {
  available: boolean;
  reason?: string;
}

export interface ChatMessage {
  role: 'user' | 'assistant';
  content: string;
}

export function fetchChatStatus(): Promise<ChatStatusResponse> {
  return apiFetch<ChatStatusResponse>('/v2/chat/status');
}

/**
 * Send chat messages and return the raw Response for SSE streaming.
 * We can't use apiFetch here because it calls .json() on the response.
 */
export async function sendChatMessage(messages: ChatMessage[]): Promise<Response> {
  const res = await fetch('/api/v2/chat', {
    method: 'POST',
    headers: buildApiHeaders(),
    body: JSON.stringify({ messages }),
  });

  if (!res.ok) {
    const text = await res.text().catch(() => 'Unknown error');
    throw new Error(text);
  }

  return res;
}
