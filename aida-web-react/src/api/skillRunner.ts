// trace:TASK-0001 | ai:claude
import { apiFetch } from './client';

// ============================================================================
// Types
// ============================================================================

export interface Warning {
  code: string;
  message: string;
  file: string;
  line: number;
  column: number | null;
  crateName: string;
  suggestion: string | null;
}

export interface WarningCategory {
  name: string;
  riskLevel: string;
  description: string;
  recommendedAction: string;
  warnings: Warning[];
}

export interface WarningsReport {
  totalWarnings: number;
  crateCounts: Record<string, number>;
  categories: WarningCategory[];
  rawOutput: string;
}

export interface ActionResponse {
  success: boolean;
  message: string;
  specId?: string;
  diffSummary?: string;
}

// ============================================================================
// API functions
// ============================================================================

/**
 * Run a skill and return the raw Response for SSE streaming.
 * Uses POST (not EventSource) so we can send parameters in the body.
 */
export async function runSkill(name: string, _params?: object): Promise<Response> {
  const res = await fetch(`/api/v2/skills/${encodeURIComponent(name)}/run`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'X-Project': 'default',
    },
  });

  if (!res.ok) {
    const text = await res.text().catch(() => 'Unknown error');
    throw new Error(text);
  }

  return res;
}

/**
 * Execute an action on skill results (auto-fix, create defect, etc.)
 */
export function executeSkillAction(
  name: string,
  action: string,
  params: Record<string, unknown>,
): Promise<ActionResponse> {
  return apiFetch<ActionResponse>(`/v2/skills/${encodeURIComponent(name)}/action`, {
    method: 'POST',
    body: JSON.stringify({ action, params }),
  });
}

export interface SkillChatMessage {
  role: 'user' | 'assistant';
  content: string;
}

/**
 * Send chat messages with skill context for AI follow-up.
 * Returns raw Response for SSE streaming.
 */
export async function sendSkillChat(
  name: string,
  messages: SkillChatMessage[],
  context: unknown,
): Promise<Response> {
  const res = await fetch(`/api/v2/skills/${encodeURIComponent(name)}/chat`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'X-Project': 'default',
    },
    body: JSON.stringify({ messages, context }),
  });

  if (!res.ok) {
    const text = await res.text().catch(() => 'Unknown error');
    throw new Error(text);
  }

  return res;
}
