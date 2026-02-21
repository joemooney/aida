import { BrowserRouter, Routes, Route } from 'react-router-dom';
import { AppLayout } from './components/layout/AppLayout';
import { DashboardPage } from './components/dashboard/DashboardPage';
import { KanbanBoard } from './components/kanban/KanbanBoard';
import { RequirementsList } from './components/list/RequirementsList';
import { SprintView } from './components/sprint/SprintView';
import { TimelineView } from './components/timeline/TimelineView';
import { SkillsView } from './components/skills/SkillsView';
import { DocsView } from './components/docs/DocsView';
import { DocFullPage } from './components/docs/DocFullPage';
import { RequirementFullPage } from './components/detail/RequirementFullPage';
import { SettingsView } from './components/settings/SettingsView';
import { QueuePage } from './components/queue/QueuePage';

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<AppLayout />}>
          <Route index element={<DashboardPage />} />
          <Route path="queue" element={<QueuePage />} />
          <Route path="board" element={<KanbanBoard />} />
          <Route path="list" element={<RequirementsList />} />
          <Route path="sprints" element={<SprintView />} />
          <Route path="timeline" element={<TimelineView />} />
          <Route path="skills" element={<SkillsView />} />
          <Route path="docs" element={<DocsView />} />
          <Route path="docs/view/*" element={<DocFullPage />} />
          <Route path="req/:id" element={<RequirementFullPage />} />
          <Route path="settings" element={<SettingsView />} />
        </Route>
      </Routes>
    </BrowserRouter>
  );
}
