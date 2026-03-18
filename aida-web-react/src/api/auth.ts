import { apiFetch } from './client';

export interface AuthConfigResponse {
  mode: 'none' | 'pin' | 'oidc' | 'both' | string;
  authEnabled: boolean;
  pinEnabled: boolean;
  oidcEnabled: boolean;
  defaultRole?: string;
}

export interface AuthLoginRequest {
  identifier: string;
  pin: string;
}

export interface AuthLoginUser {
  id: string;
  specId: string | null;
  name: string;
  email: string;
  handle: string;
  archived: boolean;
  hasPin: boolean;
  role: 'admin' | 'editor' | 'viewer' | string;
}

export interface AuthLoginResponse {
  authenticated: boolean;
  mode: string;
  sessionToken: string;
  user: AuthLoginUser;
}

export interface AuthMeUser {
  userId: string;
  handle: string;
  name: string;
  project: string;
  role: 'admin' | 'editor' | 'viewer' | string;
}

export interface AuthMeResponse {
  mode: string;
  authenticated: boolean;
  user: AuthMeUser;
}

export interface OidcStartResponse {
  mode: string;
  authorizationUrl: string;
  state: string;
}

export function fetchAuthConfig(): Promise<AuthConfigResponse> {
  return apiFetch<AuthConfigResponse>('/v2/auth/config');
}

export function login(request: AuthLoginRequest): Promise<AuthLoginResponse> {
  return apiFetch<AuthLoginResponse>('/v2/auth/login', {
    method: 'POST',
    body: JSON.stringify(request),
  });
}

export function fetchMe(): Promise<AuthMeResponse> {
  return apiFetch<AuthMeResponse>('/v2/auth/me');
}

export function logout(): Promise<void> {
  return apiFetch<void>('/v2/auth/logout', {
    method: 'POST',
  });
}

export function startOidc(): Promise<OidcStartResponse> {
  return apiFetch<OidcStartResponse>('/v2/auth/oidc/start');
}

export function finishOidc(code: string, state: string): Promise<AuthLoginResponse> {
  const params = new URLSearchParams({ code, state });
  return apiFetch<AuthLoginResponse>(`/v2/auth/oidc/callback?${params.toString()}`);
}

export interface RegisterRequest {
  handle: string;
  name: string;
  email?: string;
  pin?: string;
}

export function register(request: RegisterRequest): Promise<AuthLoginResponse> {
  return apiFetch<AuthLoginResponse>('/v2/auth/register', {
    method: 'POST',
    body: JSON.stringify(request),
  });
}

export interface SetPinRequest {
  currentPin?: string;
  newPin: string;
}

export function setPin(request: SetPinRequest): Promise<void> {
  return apiFetch<void>('/v2/auth/pin', {
    method: 'PUT',
    body: JSON.stringify(request),
  });
}
