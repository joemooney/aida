import { BrowserRouter, Routes, Route } from 'react-router-dom';
import { AppLayout } from './components/layout/AppLayout';
import { DashboardPage } from './components/dashboard/DashboardPage';
import { KanbanBoard } from './components/kanban/KanbanBoard';
import { RequirementsList } from './components/list/RequirementsList';
import { SprintView } from './components/sprint/SprintView';
import { SkillsView } from './components/skills/SkillsView';

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<AppLayout />}>
          <Route index element={<DashboardPage />} />
          <Route path="board" element={<KanbanBoard />} />
          <Route path="list" element={<RequirementsList />} />
          <Route path="sprints" element={<SprintView />} />
          <Route path="skills" element={<SkillsView />} />
        </Route>
      </Routes>
    </BrowserRouter>
  );
}
