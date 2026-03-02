import { useEffect, useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { Spinner } from '../ui/Spinner';
import { useAuth } from '../../hooks/useAuth';

export function AuthCallbackPage() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const { completeOidcLogin } = useAuth();
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const code = searchParams.get('code');
    const state = searchParams.get('state');

    if (!code || !state) {
      setError('Missing OIDC callback parameters');
      return;
    }

    void (async () => {
      try {
        await completeOidcLogin(code, state);
        navigate('/', { replace: true });
      } catch {
        setError('OIDC login failed');
      }
    })();
  }, [completeOidcLogin, navigate, searchParams]);

  return (
    <div className="flex min-h-screen items-center justify-center bg-surface px-4">
      <div className="w-full max-w-md rounded-xl border border-edge bg-surface-alt p-6 shadow-lg">
        {error ? (
          <p className="text-sm text-red-500">{error}</p>
        ) : (
          <div className="flex items-center gap-3 text-sm text-content-secondary">
            <Spinner size="sm" />
            Completing sign in...
          </div>
        )}
      </div>
    </div>
  );
}
