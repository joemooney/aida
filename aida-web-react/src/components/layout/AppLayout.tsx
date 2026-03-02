import { useState } from 'react';
import { Outlet, useSearchParams } from 'react-router-dom';
import { Sidebar } from './Sidebar';
import { Header } from './Header';
import { DetailPanel } from '../detail/DetailPanel';
import { HotkeyProvider } from '../hotkeys/HotkeyProvider';
import { GlobalHotkeys } from './GlobalHotkeys';
import { usePermissions } from '../../hooks/usePermissions';

export function AppLayout() {
  const { canWrite, role } = usePermissions();
  const [collapsed, setCollapsed] = useState(false);
  const [searchParams] = useSearchParams();
  const detailId = searchParams.get('detail');

  return (
    <HotkeyProvider>
      <GlobalHotkeys />
      <div className="flex h-screen overflow-hidden bg-surface">
        <Sidebar collapsed={collapsed} onToggle={() => setCollapsed(!collapsed)} />
        <div className="flex flex-1 flex-col overflow-hidden">
          <Header />
          {!canWrite && (
            <div className="border-b border-amber-600/30 bg-amber-500/10 px-6 py-2 text-xs text-amber-300">
              Read-only mode ({role}). You can browse data, but write actions are disabled.
            </div>
          )}
          <main className="flex-1 overflow-y-auto p-6">
            <Outlet />
          </main>
        </div>
        {detailId && <DetailPanel id={detailId} />}
      </div>
    </HotkeyProvider>
  );
}
