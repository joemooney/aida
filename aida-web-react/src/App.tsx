import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom';
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
import { ActivityPage } from './components/activity/ActivityPage';
import { ChatPage } from './components/chat/ChatPage';
import { LoginPage } from './components/auth/LoginPage';
import { AuthCallbackPage } from './components/auth/AuthCallbackPage';
import { Spinner } from './components/ui/Spinner';
import { useAuth } from './hooks/useAuth';

export default function App() {
  const { authEnabled, status } = useAuth();

  if (status === 'loading') {
    return (
      <div className="flex min-h-screen items-center justify-center bg-surface">
        <Spinner size="lg" />
      </div>
    );
  }

  return (
    <BrowserRouter>
      <Routes>
        <Route path="auth/callback" element={<AuthCallbackPage />} />
        {authEnabled && status !== 'authenticated' ? (
          <>
            <Route path="*" element={<LoginPage />} />
          </>
        ) : (
          <Route element={<AppLayout />}>
            <Route index element={<DashboardPage />} />
            <Route path="queue" element={<QueuePage />} />
            <Route path="activity" element={<ActivityPage />} />
            <Route path="board" element={<KanbanBoard />} />
            <Route path="list" element={<RequirementsList />} />
            <Route path="sprints" element={<SprintView />} />
            <Route path="timeline" element={<TimelineView />} />
            <Route path="skills" element={<SkillsView />} />
            <Route path="docs" element={<DocsView />} />
            <Route path="docs/view/*" element={<DocFullPage />} />
            <Route path="req/:id" element={<RequirementFullPage />} />
            <Route path="chat" element={<ChatPage />} />
            <Route path="settings" element={<SettingsView />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Route>
        )}
      </Routes>
    </BrowserRouter>
  );
}
