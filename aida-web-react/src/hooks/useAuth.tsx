import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import { ApiError, setAuthToken } from '../api/client';
import {
  fetchAuthConfig,
  fetchMe,
  finishOidc,
  login as loginRequest,
  logout as logoutRequest,
  startOidc,
  type AuthMeUser,
} from '../api/auth';

type AuthStatus = 'loading' | 'anonymous' | 'authenticated';

interface AuthContextValue {
  mode: string;
  authEnabled: boolean;
  pinEnabled: boolean;
  oidcEnabled: boolean;
  status: AuthStatus;
  user: AuthMeUser | null;
  error: string | null;
  login: (identifier: string, pin: string) => Promise<void>;
  beginOidcLogin: () => Promise<void>;
  completeOidcLogin: (code: string, state: string) => Promise<void>;
  logout: () => Promise<void>;
  refreshSession: () => Promise<void>;
}

const AuthContext = createContext<AuthContextValue | undefined>(undefined);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [mode, setMode] = useState('none');
  const [authEnabled, setAuthEnabled] = useState(false);
  const [pinEnabled, setPinEnabled] = useState(false);
  const [oidcEnabled, setOidcEnabled] = useState(false);
  const [status, setStatus] = useState<AuthStatus>('loading');
  const [user, setUser] = useState<AuthMeUser | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refreshSession = useCallback(async () => {
    try {
      const me = await fetchMe();
      setUser(me.user);
      setStatus('authenticated');
      setError(null);
    } catch (err) {
      setAuthToken(null);
      setUser(null);
      setStatus('anonymous');
      if (err instanceof ApiError && err.status === 401) {
        setError(null);
      } else {
        setError('Failed to verify session');
      }
    }
  }, []);

  useEffect(() => {
    let cancelled = false;

    async function bootstrap() {
      try {
        const config = await fetchAuthConfig();
        if (cancelled) return;
        setMode(config.mode);
        setAuthEnabled(config.authEnabled);
        setPinEnabled(config.pinEnabled ?? false);
        setOidcEnabled(config.oidcEnabled ?? false);

        if (!config.authEnabled) {
          setStatus('authenticated');
          setUser({
            userId: 'default',
            handle: 'default',
            name: 'Default User',
            project: 'default',
            role: 'admin',
          });
          return;
        }

        await refreshSession();
      } catch {
        if (!cancelled) {
          setStatus('anonymous');
          setError('Unable to load authentication config');
        }
      }
    }

    void bootstrap();
    return () => {
      cancelled = true;
    };
  }, [refreshSession]);

  const login = useCallback(async (identifier: string, pin: string) => {
    const res = await loginRequest({ identifier, pin });
    setAuthToken(res.sessionToken);
    await refreshSession();
  }, [refreshSession]);

  const beginOidcLogin = useCallback(async () => {
    const res = await startOidc();
    window.location.assign(res.authorizationUrl);
  }, []);

  const completeOidcLogin = useCallback(async (code: string, state: string) => {
    const res = await finishOidc(code, state);
    setAuthToken(res.sessionToken);
    await refreshSession();
  }, [refreshSession]);

  const logout = useCallback(async () => {
    try {
      await logoutRequest();
    } finally {
      setAuthToken(null);
      setUser(null);
      setStatus(authEnabled ? 'anonymous' : 'authenticated');
    }
  }, [authEnabled]);

  const value = useMemo<AuthContextValue>(() => ({
    mode,
    authEnabled,
    pinEnabled,
    oidcEnabled,
    status,
    user,
    error,
    login,
    beginOidcLogin,
    completeOidcLogin,
    logout,
    refreshSession,
  }), [authEnabled, pinEnabled, oidcEnabled, beginOidcLogin, completeOidcLogin, error, login, logout, mode, refreshSession, status, user]);

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth() {
  const ctx = useContext(AuthContext);
  if (!ctx) {
    throw new Error('useAuth must be used within AuthProvider');
  }
  return ctx;
}
