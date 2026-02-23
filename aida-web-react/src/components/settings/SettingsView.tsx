import { useState } from 'react';
import { cn } from '../../lib/utils';
import { GeneralTab } from './GeneralTab';
import { RelationshipsTab } from './RelationshipsTab';
import { TypesTab } from './TypesTab';
import { ReactionsTab } from './ReactionsTab';
import { IdsTab } from './IdsTab';
import { AdminTab } from './AdminTab';
import { ScaffoldTab } from './ScaffoldTab';

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
  const [activeTab, setActiveTab] = useState<SettingsTab>('general');

  return (
    <div className="flex flex-col gap-6 p-6">
      <h1 className="text-2xl font-bold text-content">Settings</h1>

      {/* Tab bar */}
      <div className="flex gap-1 border-b border-edge">
        {tabs.map(({ key, label }) => (
          <button
            key={key}
            onClick={() => setActiveTab(key)}
            className={cn(
              'px-4 py-2 text-sm font-medium transition-colors border-b-2 -mb-px',
              activeTab === key
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
        {activeTab === 'general' && <GeneralTab />}
        {activeTab === 'relationships' && <RelationshipsTab />}
        {activeTab === 'types' && <TypesTab />}
        {activeTab === 'reactions' && <ReactionsTab />}
        {activeTab === 'ids' && <IdsTab />}
        {activeTab === 'admin' && <AdminTab />}
        {activeTab === 'scaffold' && <ScaffoldTab />}
      </div>
    </div>
  );
}
