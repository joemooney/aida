import { useState } from 'react';
import { Button } from '../ui/Button';
import { Spinner } from '../ui/Spinner';
import { useAuth } from '../../hooks/useAuth';

export function LoginPage() {
  const { login, pinEnabled, oidcEnabled, beginOidcLogin } = useAuth();
  const [identifier, setIdentifier] = useState('');
  const [pin, setPin] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      await login(identifier.trim(), pin);
    } catch {
      setError('Invalid credentials');
    } finally {
      setSubmitting(false);
    }
  }

  async function onOidcSignIn() {
    setError(null);
    setSubmitting(true);
    try {
      await beginOidcLogin();
    } catch {
      setError('Failed to start SSO login');
      setSubmitting(false);
    }
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-surface px-4">
      <div className="w-full max-w-md rounded-xl border border-edge bg-surface-alt p-6 shadow-lg">
        <h1 className="text-xl font-semibold text-content">Sign in to AIDA</h1>

        {pinEnabled && (
          <form className="mt-5 space-y-4" onSubmit={onSubmit}>
          <div>
            <label className="mb-1 block text-sm text-content-secondary">Handle, name, or user ID</label>
            <input
              value={identifier}
              onChange={(e) => setIdentifier(e.target.value)}
              className="w-full rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
              placeholder="e.g. joe"
              autoFocus
              required
            />
          </div>

          <div>
            <label className="mb-1 block text-sm text-content-secondary">PIN (optional if user has no PIN)</label>
            <input
              value={pin}
              onChange={(e) => setPin(e.target.value)}
              className="w-full rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
              type="password"
              placeholder="Enter PIN"
            />
          </div>

          {error && !oidcEnabled && <p className="text-sm text-red-500">{error}</p>}

          <Button type="submit" className="w-full" disabled={submitting || !identifier.trim()}>
            {submitting ? <Spinner size="sm" /> : null}
            {submitting ? 'Signing in...' : 'Sign In'}
          </Button>
          </form>
        )}

        {pinEnabled && oidcEnabled && (
          <div className="my-5 flex items-center gap-3">
            <div className="h-px flex-1 bg-edge" />
            <span className="text-xs text-content-secondary">or</span>
            <div className="h-px flex-1 bg-edge" />
          </div>
        )}

        {oidcEnabled && (
          <div className={pinEnabled ? '' : 'mt-5'}>
            <Button type="button" variant="secondary" className="w-full" disabled={submitting} onClick={onOidcSignIn}>
              {submitting ? <Spinner size="sm" /> : null}
              {submitting ? 'Redirecting...' : 'Sign In with SSO'}
            </Button>
          </div>
        )}

        {error && (pinEnabled && oidcEnabled) && (
          <p className="mt-3 text-sm text-red-500">{error}</p>
        )}

        {!pinEnabled && !oidcEnabled && (
          <p className="mt-4 text-sm text-content-secondary">
            No authentication methods are configured. Contact your administrator.
          </p>
        )}
      </div>
    </div>
  );
}
