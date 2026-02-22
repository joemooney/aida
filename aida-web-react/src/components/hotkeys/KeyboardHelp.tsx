// trace:STORY-0375 | ai:claude
import { useEffect } from 'react';
import { X } from 'lucide-react';
import { useHotkeyContext } from '../../hooks/useHotkeys';

function KeyBadge({ k }: { k: string }) {
  return (
    <kbd className="inline-flex items-center justify-center min-w-[24px] rounded bg-surface-hover px-1.5 py-0.5 text-xs font-mono text-content border border-edge">
      {k}
    </kbd>
  );
}

function KeySequence({ keys }: { keys: string[] }) {
  return (
    <span className="inline-flex items-center gap-1">
      {keys.map((k, i) => (
        <span key={i} className="inline-flex items-center gap-1">
          {i > 0 && <span className="text-[10px] text-content-muted">then</span>}
          <KeyBadge k={k} />
        </span>
      ))}
    </span>
  );
}

export function KeyboardHelp() {
  const { getBindings, setHelpOpen } = useHotkeyContext();
  const bindings = getBindings();

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        setHelpOpen(false);
      }
    }
    document.addEventListener('keydown', handleKeyDown, true);
    return () => document.removeEventListener('keydown', handleKeyDown, true);
  }, [setHelpOpen]);

  // Group bindings by category
  const groups = new Map<string, typeof bindings>();
  for (const b of bindings) {
    if (b.enabled === false) continue;
    const list = groups.get(b.category) ?? [];
    list.push(b);
    groups.set(b.category, list);
  }

  return (
    <>
      {/* Backdrop */}
      <div
        className="fixed inset-0 bg-black/40 z-[60] animate-fade-in"
        onClick={() => setHelpOpen(false)}
      />

      {/* Modal */}
      <div className="fixed inset-0 z-[61] flex items-center justify-center p-4">
        <div className="w-full max-w-lg max-h-[80vh] rounded-xl border border-edge bg-surface-alt shadow-2xl shadow-black/40 flex flex-col animate-fade-in">
          {/* Header */}
          <div className="flex items-center justify-between px-5 py-4 border-b border-edge">
            <h2 className="text-base font-semibold text-content">Keyboard Shortcuts</h2>
            <button
              onClick={() => setHelpOpen(false)}
              className="flex h-7 w-7 items-center justify-center rounded-lg text-content-muted hover:text-content hover:bg-surface-hover transition-colors cursor-pointer"
            >
              <X className="h-4 w-4" />
            </button>
          </div>

          {/* Body */}
          <div className="flex-1 overflow-y-auto px-5 py-4 space-y-5">
            {Array.from(groups.entries()).map(([category, items]) => (
              <div key={category}>
                <h3 className="text-xs font-medium uppercase tracking-wider text-content-muted mb-2">
                  {category}
                </h3>
                <div className="space-y-1.5">
                  {items.map((b) => (
                    <div
                      key={b.id}
                      className="flex items-center justify-between py-1"
                    >
                      <span className="text-sm text-content">{b.description}</span>
                      <KeySequence keys={b.keys} />
                    </div>
                  ))}
                </div>
              </div>
            ))}
          </div>

          {/* Footer */}
          <div className="px-5 py-3 border-t border-edge">
            <p className="text-xs text-content-muted">
              Press <KeyBadge k="?" /> to toggle this help. Shortcuts are disabled in text fields.
            </p>
          </div>
        </div>
      </div>
    </>
  );
}
