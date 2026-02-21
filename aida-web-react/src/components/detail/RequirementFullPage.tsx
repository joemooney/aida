import { useParams, Link } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { useRequirement } from '../../hooks/useRequirements';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { DetailHeader } from './DetailHeader';
import { DetailBody } from './DetailBody';
import { DetailComments } from './DetailComments';

export function RequirementFullPage() {
  const { id } = useParams();
  const { data: requirement, isLoading, error } = useRequirement(id ?? '');

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <Spinner size="lg" />
      </div>
    );
  }

  if (error || !requirement) {
    return (
      <EmptyState
        title="Requirement not found"
        description={`Could not load "${id}".`}
      />
    );
  }

  return (
    <div className="max-w-2xl mx-auto">
      {/* Back link */}
      <div className="mb-4">
        <Link
          to={`/board?detail=${id}`}
          className="inline-flex items-center gap-1.5 text-xs text-content-muted hover:text-content transition-colors"
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          Back to board
        </Link>
      </div>

      {/* Reuse the same detail components */}
      <div className="rounded-xl border border-edge bg-surface-alt flex flex-col">
        <DetailHeader requirement={requirement} onClose={() => {}} hideClose />
        <DetailBody requirement={requirement} />
        <DetailComments
          requirementId={requirement.spec_id ?? requirement.id}
          comments={requirement.comments ?? []}
        />
      </div>
    </div>
  );
}
