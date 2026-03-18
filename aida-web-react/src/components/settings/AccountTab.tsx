import { useState } from 'react';
import { Button } from '../ui/Button';
import { Spinner } from '../ui/Spinner';
import { useAuth } from '../../hooks/useAuth';
import { setPin } from '../../api/auth';

export function AccountTab() {
  const { user, pinEnabled } = useAuth();
  const [currentPin, setCurrentPin] = useState('');
  const [newPin, setNewPin] = useState('');
  const [confirmPin, setConfirmPin] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [success, setSuccess] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function onSetPin(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setSuccess(null);

    if (newPin !== confirmPin) {
      setError('PINs do not match');
      return;
    }
    if (newPin.length < 1) {
      setError('PIN cannot be empty');
      return;
    }

    setSubmitting(true);
    try {
      await setPin({ currentPin: currentPin || undefined, newPin });
      setSuccess('PIN updated successfully');
      setCurrentPin('');
      setNewPin('');
      setConfirmPin('');
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to update PIN');
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="space-y-6">
      {/* Profile info */}
      <section>
        <h2 className="text-lg font-semibold text-content">Profile</h2>
        <div className="mt-3 rounded-lg border border-edge bg-surface p-4">
          <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-sm">
            <dt className="text-content-secondary">Handle</dt>
            <dd className="font-mono text-content">{user?.handle ?? '—'}</dd>
            <dt className="text-content-secondary">Name</dt>
            <dd className="text-content">{user?.name ?? '—'}</dd>
            <dt className="text-content-secondary">Role</dt>
            <dd className="text-content capitalize">{user?.role ?? '—'}</dd>
          </dl>
        </div>
      </section>

      {/* PIN management */}
      {pinEnabled && (
        <section>
          <h2 className="text-lg font-semibold text-content">Change PIN</h2>
          <form className="mt-3 max-w-sm space-y-4" onSubmit={onSetPin}>
            <div>
              <label className="mb-1 block text-sm text-content-secondary">
                Current PIN
              </label>
              <input
                value={currentPin}
                onChange={(e) => setCurrentPin(e.target.value)}
                className="w-full rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
                type="password"
                placeholder="Leave blank if no PIN set"
              />
            </div>
            <div>
              <label className="mb-1 block text-sm text-content-secondary">
                New PIN
              </label>
              <input
                value={newPin}
                onChange={(e) => setNewPin(e.target.value)}
                className="w-full rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
                type="password"
                placeholder="Enter new PIN"
                required
              />
            </div>
            <div>
              <label className="mb-1 block text-sm text-content-secondary">
                Confirm PIN
              </label>
              <input
                value={confirmPin}
                onChange={(e) => setConfirmPin(e.target.value)}
                className="w-full rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
                type="password"
                placeholder="Confirm new PIN"
                required
              />
            </div>

            {error && <p className="text-sm text-red-500">{error}</p>}
            {success && <p className="text-sm text-green-500">{success}</p>}

            <Button type="submit" disabled={submitting || !newPin}>
              {submitting ? <Spinner size="sm" /> : null}
              {submitting ? 'Updating...' : 'Update PIN'}
            </Button>
          </form>
        </section>
      )}
    </div>
  );
}
