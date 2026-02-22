// trace:STORY-0375 | ai:claude
import { useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import { useHotkeys, useHotkeyContext, type HotkeyBinding } from './useHotkeys';
import { useDetailPanel } from './useDetailPanel';

export function useGlobalHotkeys(): void {
  const navigate = useNavigate();
  const { detailId, close } = useDetailPanel();
  const { setHelpOpen } = useHotkeyContext();

  const bindings: HotkeyBinding[] = useMemo(
    () => [
      // Global
      {
        id: 'global:help',
        description: 'Show keyboard shortcuts',
        category: 'Global',
        keys: ['?'],
        handler: () => setHelpOpen(true),
      },
      {
        id: 'global:search',
        description: 'Focus search',
        category: 'Global',
        keys: ['/'],
        handler: () => {
          const input = document.querySelector<HTMLInputElement>('header input');
          input?.focus();
        },
      },
      {
        id: 'global:escape',
        description: 'Close panel',
        category: 'Global',
        keys: ['Escape'],
        ignoreInInput: false,
        handler: () => {
          if (detailId) close();
        },
        enabled: !!detailId,
      },

      // Navigation chords
      {
        id: 'nav:dashboard',
        description: 'Go to Dashboard',
        category: 'Navigation',
        keys: ['g', 'd'],
        handler: () => navigate('/'),
      },
      {
        id: 'nav:queue',
        description: 'Go to My Queue',
        category: 'Navigation',
        keys: ['g', 'q'],
        handler: () => navigate('/queue'),
      },
      {
        id: 'nav:board',
        description: 'Go to Kanban Board',
        category: 'Navigation',
        keys: ['g', 'b'],
        handler: () => navigate('/board'),
      },
      {
        id: 'nav:list',
        description: 'Go to List View',
        category: 'Navigation',
        keys: ['g', 'l'],
        handler: () => navigate('/list'),
      },
      {
        id: 'nav:sprints',
        description: 'Go to Sprints',
        category: 'Navigation',
        keys: ['g', 's'],
        handler: () => navigate('/sprints'),
      },
      {
        id: 'nav:timeline',
        description: 'Go to Timeline',
        category: 'Navigation',
        keys: ['g', 't'],
        handler: () => navigate('/timeline'),
      },
      {
        id: 'nav:chat',
        description: 'Go to Chat',
        category: 'Navigation',
        keys: ['g', 'c'],
        handler: () => navigate('/chat'),
      },
      {
        id: 'nav:settings',
        description: 'Go to Settings',
        category: 'Navigation',
        keys: ['g', 'x'],
        handler: () => navigate('/settings'),
      },
    ],
    [navigate, detailId, close, setHelpOpen],
  );

  useHotkeys(bindings, [navigate, detailId, close, setHelpOpen]);
}
