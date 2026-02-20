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
  const { close } = useDetailPanel();
  const { data: requirement, isLoading, error } = useRequirement(id);

  // Close on Escape
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') close();
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [close]);

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
            <DetailBody requirement={requirement} />
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
