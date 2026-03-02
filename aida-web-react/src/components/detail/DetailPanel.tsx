import { useEffect } from 'react';
import { useRequirement } from '../../hooks/useRequirements';
import { useDetailPanel } from '../../hooks/useDetailPanel';
import { Spinner } from '../ui/Spinner';
import { DetailHeader } from './DetailHeader';
import { DetailBody } from './DetailBody';
import { DetailComments } from './DetailComments';

interface DetailPanelProps {
  id: string;
}

export function DetailPanel({ id }: DetailPanelProps) {
  const { close, detailId, detailMode, open } = useDetailPanel();
  const { data: requirement, isLoading, error } = useRequirement(id);

  const autoEditDescription = detailMode === 'edit-desc';

  // ESC closes detail panel unless inner controls already handled it.
  // This allows: ESC inside editor -> exit edit; ESC again -> close panel.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const tag = target?.tagName?.toLowerCase();
      const isTextInput = tag === 'input' || tag === 'textarea' || tag === 'select';

      if (e.key === 'Enter' && !e.defaultPrevented && detailId && detailMode !== 'edit-desc') {
        if (!isTextInput && tag !== 'button' && tag !== 'a') {
          e.preventDefault();
          open(detailId, { startInDescriptionEdit: true });
          return;
        }
      }

      if (e.key === 'Escape' && !e.defaultPrevented) {
        close();
      }
    };
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [close, detailId, detailMode, open]);

  return (
    <>
      {/* Backdrop */}
      <div
        className="fixed inset-0 bg-black/40 z-40 animate-fade-in"
        onClick={close}
      />

      {/* Panel */}
      <div className="fixed top-0 right-0 bottom-0 z-50 w-full max-w-lg bg-surface-alt border-l border-edge flex flex-col animate-slide-in-right shadow-2xl shadow-black/40">
        {isLoading ? (
          <div className="flex items-center justify-center flex-1">
            <Spinner size="lg" />
          </div>
        ) : error || !requirement ? (
          <div className="flex flex-col items-center justify-center flex-1 text-content-muted">
            <p className="text-sm">Failed to load requirement</p>
            <button
              onClick={close}
              className="mt-3 text-xs text-accent hover:text-accent-hover cursor-pointer"
            >
              Close panel
            </button>
          </div>
        ) : (
          <>
            <DetailHeader requirement={requirement} onClose={close} />
            <DetailBody requirement={requirement} autoEditDescription={autoEditDescription} />
            <DetailComments
              requirementId={requirement.spec_id ?? requirement.id}
              comments={requirement.comments ?? []}
            />
          </>
        )}
      </div>
    </>
  );
}
