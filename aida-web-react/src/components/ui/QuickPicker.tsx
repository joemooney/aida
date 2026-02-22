// trace:STORY-0375 | ai:claude
import { useState, useEffect, useRef, type RefObject } from 'react';
import { cn } from '../../lib/utils';

interface QuickPickerProps {
  anchorRef: RefObject<HTMLElement | null>;
  options: string[];
  label: string;
  onSelect: (value: string) => void;
  onClose: () => void;
}

export function QuickPicker({ anchorRef, options, label, onSelect, onClose }: QuickPickerProps) {
  const [activeIndex, setActiveIndex] = useState(0);
  const popoverRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<{ top: number; left: number }>({ top: 0, left: 0 });

  // Position the popover near the anchor element
  useEffect(() => {
    const anchor = anchorRef.current;
    if (!anchor) return;
    const rect = anchor.getBoundingClientRect();
    setPosition({
      top: rect.bottom + 4,
      left: rect.left + 40,
    });
  }, [anchorRef]);

  // Keyboard navigation
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      switch (e.key) {
        case 'ArrowDown':
        case 'j':
          e.preventDefault();
          e.stopPropagation();
          setActiveIndex((i) => Math.min(i + 1, options.length - 1));
          break;
        case 'ArrowUp':
        case 'k':
          e.preventDefault();
          e.stopPropagation();
          setActiveIndex((i) => Math.max(i - 1, 0));
          break;
        case 'Enter':
          e.preventDefault();
          e.stopPropagation();
          if (options[activeIndex]) onSelect(options[activeIndex]);
          break;
        case 'Escape':
          e.preventDefault();
          e.stopPropagation();
          onClose();
          break;
      }
    }
    // Use capture phase to intercept before the hotkey system
    document.addEventListener('keydown', handleKeyDown, true);
    return () => document.removeEventListener('keydown', handleKeyDown, true);
  }, [options, activeIndex, onSelect, onClose]);

  // Close on outside click
  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (popoverRef.current && !popoverRef.current.contains(e.target as Node)) {
        onClose();
      }
    }
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [onClose]);

  if (options.length === 0) {
    onClose();
    return null;
  }

  return (
    <div
      ref={popoverRef}
      className="fixed z-50 min-w-[160px] rounded-lg border border-edge bg-surface-alt shadow-xl shadow-black/20 py-1 animate-fade-in"
      style={{ top: position.top, left: position.left }}
    >
      <div className="px-3 py-1.5 text-[10px] font-medium uppercase tracking-wider text-content-muted border-b border-edge mb-1">
        {label}
      </div>
      {options.map((option, i) => (
        <button
          key={option}
          onClick={() => onSelect(option)}
          className={cn(
            'flex w-full items-center px-3 py-1.5 text-sm text-left transition-colors cursor-pointer',
            i === activeIndex
              ? 'bg-accent/10 text-accent'
              : 'text-content hover:bg-surface-hover',
          )}
        >
          {option}
        </button>
      ))}
    </div>
  );
}
