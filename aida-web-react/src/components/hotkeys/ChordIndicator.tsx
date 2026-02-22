// trace:STORY-0375 | ai:claude
interface ChordIndicatorProps {
  chord: string | null;
}

export function ChordIndicator({ chord }: ChordIndicatorProps) {
  if (!chord) return null;

  return (
    <div className="fixed bottom-4 right-4 z-50 rounded-lg border border-edge bg-surface-alt px-3 py-2 shadow-lg shadow-black/20 animate-fade-in">
      <span className="text-sm font-mono text-content-muted">
        <kbd className="rounded bg-surface-hover px-1.5 py-0.5 text-xs font-mono text-content border border-edge">
          {chord}
        </kbd>
        <span className="ml-1.5 text-content-muted">...</span>
      </span>
    </div>
  );
}
