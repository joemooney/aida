import { useState } from 'react';
import { cn } from '../../lib/utils';
import { GeneralTab } from './GeneralTab';
import { RelationshipsTab } from './RelationshipsTab';
import { TypesTab } from './TypesTab';
import { ReactionsTab } from './ReactionsTab';
import { IdsTab } from './IdsTab';
import { AdminTab } from './AdminTab';
import { ScaffoldTab } from './ScaffoldTab';
import { usePermissions } from '../../hooks/usePermissions';

type SettingsTab = 'general' | 'relationships' | 'types' | 'reactions' | 'ids' | 'admin' | 'scaffold';

const tabs: { key: SettingsTab; label: string }[] = [
  { key: 'general', label: 'General' },
  { key: 'relationships', label: 'Relationships' },
  { key: 'types', label: 'Types' },
  { key: 'reactions', label: 'Reactions' },
  { key: 'ids', label: 'IDs & Prefixes' },
  { key: 'admin', label: 'Admin' },
  { key: 'scaffold', label: 'New Project' },
];

export function SettingsView() {
  const { canWrite, canAdmin } = usePermissions();
  const availableTabs = tabs.filter((tab) => {
    if (tab.key === 'admin') return canAdmin;
    if (tab.key === 'scaffold') return canAdmin;
    if (tab.key === 'general') return true;
    return canWrite;
  });

  const firstTab = availableTabs[0]?.key ?? 'general';
  const [activeTab, setActiveTab] = useState<SettingsTab>(firstTab);
  const resolvedActiveTab = availableTabs.some((t) => t.key === activeTab)
    ? activeTab
    : firstTab;

  return (
    <div className="flex flex-col gap-6 p-6">
      <h1 className="text-2xl font-bold text-content">Settings</h1>
      {!canWrite && (
        <div className="rounded-lg border border-amber-600/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-300">
          You have read-only access. Settings changes are disabled.
        </div>
      )}

      {/* Tab bar */}
      <div className="flex gap-1 border-b border-edge">
        {availableTabs.map(({ key, label }) => (
          <button
            key={key}
            onClick={() => setActiveTab(key as SettingsTab)}
            className={cn(
              'px-4 py-2 text-sm font-medium transition-colors border-b-2 -mb-px',
              resolvedActiveTab === key
                ? 'border-accent text-accent'
                : 'border-transparent text-content-secondary hover:text-content hover:border-edge',
            )}
          >
            {label}
          </button>
        ))}
      </div>

      {/* Tab content */}
      <div>
        {resolvedActiveTab === 'general' && <GeneralTab />}
        {resolvedActiveTab === 'relationships' && <RelationshipsTab />}
        {resolvedActiveTab === 'types' && <TypesTab />}
        {resolvedActiveTab === 'reactions' && <ReactionsTab />}
        {resolvedActiveTab === 'ids' && <IdsTab />}
        {resolvedActiveTab === 'admin' && <AdminTab />}
        {resolvedActiveTab === 'scaffold' && <ScaffoldTab />}
      </div>
    </div>
  );
}
