// trace:STORY-0375 | ai:claude
import { createContext, useContext, useEffect, useRef } from 'react';

export interface HotkeyBinding {
  id: string;
  description: string;
  category: string;
  keys: string[];          // single: ["j"], modifier: ["ctrl+k"], chord: ["g","l"]
  handler: () => void;
  ignoreInInput?: boolean; // default true
  enabled?: boolean;       // default true
}

export interface HotkeyContextValue {
  register: (ref: React.RefObject<HotkeyBinding[]>) => () => void;
  getBindings: () => HotkeyBinding[];
  pendingChord: string | null;
  helpOpen: boolean;
  setHelpOpen: (open: boolean) => void;
}

export const HotkeyContext = createContext<HotkeyContextValue | null>(null);

export function useHotkeyContext(): HotkeyContextValue {
  const ctx = useContext(HotkeyContext);
  if (!ctx) throw new Error('useHotkeyContext must be used within HotkeyProvider');
  return ctx;
}

export function useHotkeys(bindings: HotkeyBinding[]): void {
  const ctx = useHotkeyContext();
  const bindingsRef = useRef<HotkeyBinding[]>(bindings);
  bindingsRef.current = bindings;

  useEffect(() => {
    const unregister = ctx.register(bindingsRef);
    return unregister;
    // Only register/unregister on mount/unmount — bindings are read via ref
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ctx.register]);
}

export function isInputFocused(): boolean {
  const el = document.activeElement;
  if (!el) return false;
  const tag = el.tagName;
  if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return true;
  if ((el as HTMLElement).isContentEditable) return true;
  return false;
}
