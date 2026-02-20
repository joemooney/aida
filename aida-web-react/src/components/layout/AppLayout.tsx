import { useState } from 'react';
import { Outlet, useSearchParams } from 'react-router-dom';
import { Sidebar } from './Sidebar';
import { Header } from './Header';
import { DetailPanel } from '../detail/DetailPanel';

export function AppLayout() {
  const [collapsed, setCollapsed] = useState(false);
  const [searchParams] = useSearchParams();
  const detailId = searchParams.get('detail');

  return (
    <div className="flex h-screen overflow-hidden bg-surface">
      <Sidebar collapsed={collapsed} onToggle={() => setCollapsed(!collapsed)} />
      <div className="flex flex-1 flex-col overflow-hidden">
        <Header />
        <main className="flex-1 overflow-y-auto p-6">
          <Outlet />
        </main>
      </div>
      {detailId && <DetailPanel id={detailId} />}
    </div>
  );
}
