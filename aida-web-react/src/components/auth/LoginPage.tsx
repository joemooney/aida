import { useState } from 'react';
import { Button } from '../ui/Button';
import { Spinner } from '../ui/Spinner';
import { useAuth } from '../../hooks/useAuth';
import { register } from '../../api/auth';
import { setAuthToken } from '../../api/client';

export function LoginPage() {
  const { login, pinEnabled, oidcEnabled, beginOidcLogin, refreshSession } = useAuth();
  const [identifier, setIdentifier] = useState('');
  const [pin, setPin] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [showRegister, setShowRegister] = useState(false);

  // Registration fields
  const [regHandle, setRegHandle] = useState('');
  const [regName, setRegName] = useState('');
  const [regEmail, setRegEmail] = useState('');
  const [regPin, setRegPin] = useState('');

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

  async function onRegister(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      const res = await register({
        handle: regHandle.trim(),
        name: regName.trim(),
        email: regEmail.trim() || undefined,
        pin: regPin || undefined,
      });
      setAuthToken(res.sessionToken);
      await refreshSession();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Registration failed');
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
        <h1 className="text-xl font-semibold text-content">
          {showRegister ? 'Create Account' : 'Sign in to AIDA'}
        </h1>

        {showRegister ? (
          <form className="mt-5 space-y-4" onSubmit={onRegister}>
            <div>
              <label className="mb-1 block text-sm text-content-secondary">Handle</label>
              <input
                value={regHandle}
                onChange={(e) => setRegHandle(e.target.value)}
                className="w-full rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
                placeholder="e.g. joe"
                autoFocus
                required
              />
            </div>
            <div>
              <label className="mb-1 block text-sm text-content-secondary">Full Name</label>
              <input
                value={regName}
                onChange={(e) => setRegName(e.target.value)}
                className="w-full rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
                placeholder="Joe Mooney"
                required
              />
            </div>
            <div>
              <label className="mb-1 block text-sm text-content-secondary">Email (optional)</label>
              <input
                value={regEmail}
                onChange={(e) => setRegEmail(e.target.value)}
                className="w-full rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
                placeholder="joe@example.com"
                type="email"
              />
            </div>
            <div>
              <label className="mb-1 block text-sm text-content-secondary">PIN (optional)</label>
              <input
                value={regPin}
                onChange={(e) => setRegPin(e.target.value)}
                className="w-full rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
                type="password"
                placeholder="Choose a PIN"
              />
            </div>

            {error && <p className="text-sm text-red-500">{error}</p>}

            <Button type="submit" className="w-full" disabled={submitting || !regHandle.trim() || !regName.trim()}>
              {submitting ? <Spinner size="sm" /> : null}
              {submitting ? 'Creating...' : 'Create Account'}
            </Button>

            <p className="text-center text-sm text-content-secondary">
              Already have an account?{' '}
              <button type="button" className="text-accent hover:underline" onClick={() => { setShowRegister(false); setError(null); }}>
                Sign in
              </button>
            </p>
          </form>
        ) : (
          <>
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
                  <label className="mb-1 block text-sm text-content-secondary">PIN (optional if no PIN set)</label>
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

            {error && pinEnabled && oidcEnabled && (
              <p className="mt-3 text-sm text-red-500">{error}</p>
            )}

            {pinEnabled && (
              <p className="mt-4 text-center text-sm text-content-secondary">
                No account?{' '}
                <button type="button" className="text-accent hover:underline" onClick={() => { setShowRegister(true); setError(null); }}>
                  Create one
                </button>
              </p>
            )}
          </>
        )}
      </div>
    </div>
  );
}
