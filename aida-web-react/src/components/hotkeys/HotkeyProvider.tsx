// trace:STORY-0375 | ai:claude
import { useState, useCallback, useRef, useEffect, type ReactNode } from 'react';
import { HotkeyContext, isInputFocused, type HotkeyBinding } from '../../hooks/useHotkeys';
import { ChordIndicator } from './ChordIndicator';
import { KeyboardHelp } from './KeyboardHelp';

function normalizeKeyEvent(e: KeyboardEvent): string {
  const parts: string[] = [];
  if (e.ctrlKey) parts.push('ctrl');
  if (e.metaKey) parts.push('meta');
  if (e.altKey) parts.push('alt');
  if (e.shiftKey && e.key.length > 1) parts.push('shift');

  let key = e.key;
  if (key === ' ') key = 'Space';
  if (key.length === 1) key = key.toLowerCase();

  // Shift+/ produces '?' — use the produced character directly
  if (e.shiftKey && key.length === 1) {
    parts.length = 0; // clear modifiers for shifted single chars like ?
    if (e.ctrlKey) parts.push('ctrl');
    if (e.metaKey) parts.push('meta');
    if (e.altKey) parts.push('alt');
  }

  parts.push(key);
  return parts.join('+');
}

interface Props {
  children: ReactNode;
}

export function HotkeyProvider({ children }: Props) {
  const [helpOpen, setHelpOpen] = useState(false);
  const [pendingChord, setPendingChord] = useState<string | null>(null);
  const [allBindings, setAllBindings] = useState<HotkeyBinding[]>([]);
  const bindingsMapRef = useRef(new Map<object, HotkeyBinding[]>());
  const chordTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingChordRef = useRef<string | null>(null);

  // Keep ref in sync
  useEffect(() => {
    pendingChordRef.current = pendingChord;
  }, [pendingChord]);

  const rebuildBindings = useCallback(() => {
    const all: HotkeyBinding[] = [];
    for (const bindings of bindingsMapRef.current.values()) {
      all.push(...bindings);
    }
    setAllBindings(all);
  }, []);

  const register = useCallback(
    (bindings: HotkeyBinding[]) => {
      const key = {};
      bindingsMapRef.current.set(key, bindings);
      rebuildBindings();
      return () => {
        bindingsMapRef.current.delete(key);
        rebuildBindings();
      };
    },
    [rebuildBindings],
  );

  // Single keydown listener
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      const pressed = normalizeKeyEvent(e);
      const currentChord = pendingChordRef.current;
      const inputActive = isInputFocused();

      // Get enabled bindings
      const enabled = allBindings.filter((b) => b.enabled !== false);

      if (currentChord) {
        // We're in chord mode — look for second key match
        if (chordTimeoutRef.current) {
          clearTimeout(chordTimeoutRef.current);
          chordTimeoutRef.current = null;
        }

        const match = enabled.find(
          (b) =>
            b.keys.length === 2 &&
            b.keys[0] === currentChord &&
            b.keys[1] === pressed,
        );

        setPendingChord(null);
        pendingChordRef.current = null;

        if (match) {
          const skip = (match.ignoreInInput !== false) && inputActive;
          if (!skip) {
            e.preventDefault();
            match.handler();
          }
        }
        return;
      }

      // Not in chord mode
      // Check if this key starts a chord
      const startsChord = enabled.some(
        (b) => b.keys.length === 2 && b.keys[0] === pressed,
      );

      // Check for single-key match
      const singleMatch = enabled.find(
        (b) => b.keys.length === 1 && b.keys[0] === pressed,
      );

      if (startsChord) {
        // Check if we should ignore in input
        const anyChordIgnoresInput = enabled.some(
          (b) => b.keys.length === 2 && b.keys[0] === pressed && b.ignoreInInput !== false,
        );
        if (anyChordIgnoresInput && inputActive) {
          // Let single key match handle it if exists, or do nothing
          if (singleMatch) {
            const skip = (singleMatch.ignoreInInput !== false) && inputActive;
            if (!skip) {
              e.preventDefault();
              singleMatch.handler();
            }
          }
          return;
        }

        e.preventDefault();
        setPendingChord(pressed);
        pendingChordRef.current = pressed;
        chordTimeoutRef.current = setTimeout(() => {
          setPendingChord(null);
          pendingChordRef.current = null;
        }, 1000);
        return;
      }

      if (singleMatch) {
        const skip = (singleMatch.ignoreInInput !== false) && inputActive;
        if (!skip) {
          e.preventDefault();
          singleMatch.handler();
        }
      }
    }

    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [allBindings]);

  // Cleanup chord timeout
  useEffect(() => {
    return () => {
      if (chordTimeoutRef.current) clearTimeout(chordTimeoutRef.current);
    };
  }, []);

  return (
    <HotkeyContext.Provider
      value={{ register, bindings: allBindings, pendingChord, helpOpen, setHelpOpen }}
    >
      {children}
      <ChordIndicator chord={pendingChord} />
      {helpOpen && <KeyboardHelp />}
    </HotkeyContext.Provider>
  );
}
