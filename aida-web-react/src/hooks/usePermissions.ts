import { useAuth } from './useAuth';

export function usePermissions() {
  const { authEnabled, user } = useAuth();
  const role = authEnabled ? (user?.role ?? 'viewer') : 'admin';
  const canWrite = role === 'admin' || role === 'editor';
  const canAdmin = role === 'admin';

  return {
    role,
    canWrite,
    canAdmin,
  };
}

export function requireWrite(canWrite: boolean): void {
  if (!canWrite) {
    throw new Error('You do not have permission to modify data.');
  }
}

export function requireAdmin(canAdmin: boolean): void {
  if (!canAdmin) {
    throw new Error('You do not have admin permission.');
  }
}
