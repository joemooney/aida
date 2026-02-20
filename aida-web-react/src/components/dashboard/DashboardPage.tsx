import { useRequirements } from '../../hooks/useRequirements';
import { Spinner } from '../ui/Spinner';
import { EmptyState } from '../ui/EmptyState';
import { MetricsCards } from './MetricsCards';
import { StatusChart } from './StatusChart';
import { FeatureProgress } from './FeatureProgress';
import { LayoutDashboard } from 'lucide-react';

export function DashboardPage() {
  const { data: requirements, isLoading, error } = useRequirements();

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <Spinner size="lg" />
      </div>
    );
  }

  if (error) {
    return (
      <EmptyState
        title="Failed to load requirements"
        description="Make sure the AIDA server is running on port 8080."
      />
    );
  }

  const reqs = requirements ?? [];
  // Filter out stateless types (Folder, Meta) from metrics
  const stateful = reqs.filter((r) => r.req_type !== 'Folder' && r.req_type !== 'Meta');

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-xl font-semibold text-content">Dashboard</h1>
        <p className="text-sm text-content-secondary mt-1">
          Overview of {stateful.length} requirements across your project.
        </p>
      </div>

      {stateful.length === 0 ? (
        <EmptyState
          icon={<LayoutDashboard className="h-10 w-10" />}
          title="No requirements yet"
          description="Add requirements via the CLI to see them here."
        />
      ) : (
        <>
          <MetricsCards requirements={stateful} />
          <div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
            <StatusChart requirements={stateful} />
            <FeatureProgress requirements={stateful} />
          </div>
        </>
      )}
    </div>
  );
}
