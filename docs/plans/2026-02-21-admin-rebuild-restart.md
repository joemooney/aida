# Admin Rebuild & Restart: Dev-Mode Server Rebuild via Web UI

## Related Requirements
- TASK-0373

## Summary

Added a dev-mode-only Admin tab in the Settings view that allows triggering `cargo build -p aida-server` from the browser, streaming build output in real-time via SSE, and auto-restarting the server on success. Gated behind `AIDA_DEV_MODE=1` environment variable.

## Files

### New Files (4)
| File | Description |
|------|-------------|
| `aida-server/src/admin.rs` | Admin endpoints: status + SSE rebuild stream |
| `aida-web-react/src/api/admin.ts` | API client types + fetchAdminStatus |
| `aida-web-react/src/hooks/useAdmin.ts` | useAdminStatus + useRebuild hooks |
| `aida-web-react/src/components/settings/AdminTab.tsx` | Admin tab UI component |

### Modified Files (2)
| File | Change |
|------|--------|
| `aida-server/src/main.rs` | Add `mod admin`, create AdminState, merge router |
| `aida-web-react/src/components/settings/SettingsView.tsx` | Add Admin tab |

## Architecture

- **Backend**: `AdminState` with `AtomicBool` for concurrent build protection, SSE via `tokio::sync::mpsc` + `ReceiverStream`, workspace root auto-detection
- **Frontend**: React Query for status polling, `EventSource` for SSE build streaming, auto-reconnect polling after restart

## Status
Completed
