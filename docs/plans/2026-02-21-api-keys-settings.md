# API Keys Settings for AIDA Chat

## Related Requirements
- TASK-0374: Runtime API key management via Settings Admin UI

## Status
Completed

## Summary
Added runtime API key management to the Settings Admin tab, allowing PMs/stakeholders to configure the Anthropic API key without shell access to environment variables.

### Changes Made
- **aida-server/src/admin.rs**: Added `api_keys: RwLock<HashMap>` and `api_key_sources` to AdminState, pre-populated from env. Added GET/PUT/DELETE `/api/v2/admin/api-keys` endpoints with masked key display.
- **aida-server/src/chat.rs**: Created `ChatState` wrapper combining `ServerState` + `AdminState`. Updated handlers to read API key from runtime store.
- **aida-server/src/main.rs**: Passed `admin_state` to `create_chat_router()`.
- **aida-web-react/src/api/admin.ts**: Added `fetchApiKeys`, `setApiKey`, `deleteApiKey` API functions.
- **aida-web-react/src/hooks/useAdmin.ts**: Added `useApiKeys`, `useSetApiKey`, `useDeleteApiKey` hooks with query invalidation.
- **aida-web-react/src/components/settings/AdminTab.tsx**: Added `ApiKeysCard` component with set/update/clear UI.

### Design Decisions
- Keys stored in-memory only (not persisted to disk) for security
- Env var fallback: if runtime key is cleared, falls back to env var
- Masked display: first 7 + last 4 chars shown (e.g., "sk-ant-...xYz9")
- Auto-invalidates chat-status query on key changes
