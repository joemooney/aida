import { useState, useEffect } from 'react';
import { Plus, X } from 'lucide-react';
import { useIdConfig, useUpdateIdConfig, usePrefixes, useUpdatePrefixes } from '../../hooks/useSettings';
import { Spinner } from '../ui/Spinner';
import type { IdFormat, NumberingStrategy, RequirementTypeDefinition } from '@shared/types';

export function IdsTab() {
  const { data: idConfig, isLoading: idLoading } = useIdConfig();
  const { data: prefixConfig, isLoading: prefixLoading } = usePrefixes();
  const updateIdMutation = useUpdateIdConfig();
  const updatePrefixMutation = useUpdatePrefixes();

  // ID config state
  const [format, setFormat] = useState<IdFormat>('SingleLevel');
  const [numbering, setNumbering] = useState<NumberingStrategy>('Global');
  const [digits, setDigits] = useState(3);
  const [reqTypes, setReqTypes] = useState<RequirementTypeDefinition[]>([]);
  const [newTypeName, setNewTypeName] = useState('');
  const [newTypePrefix, setNewTypePrefix] = useState('');

  // Prefix config state
  const [restrictPrefixes, setRestrictPrefixes] = useState(false);
  const [allowedPrefixes, setAllowedPrefixes] = useState<string[]>([]);
  const [newPrefix, setNewPrefix] = useState('');

  useEffect(() => {
    if (idConfig) {
      setFormat(idConfig.format);
      setNumbering(idConfig.numbering);
      setDigits(idConfig.digits);
      setReqTypes(idConfig.requirement_types);
    }
  }, [idConfig]);

  useEffect(() => {
    if (prefixConfig) {
      setRestrictPrefixes(prefixConfig.restrict_prefixes);
      setAllowedPrefixes(prefixConfig.allowed_prefixes);
    }
  }, [prefixConfig]);

  if (idLoading || prefixLoading) {
    return <div className="flex justify-center py-12"><Spinner /></div>;
  }

  const idChanged = idConfig && (
    format !== idConfig.format ||
    numbering !== idConfig.numbering ||
    digits !== idConfig.digits ||
    JSON.stringify(reqTypes) !== JSON.stringify(idConfig.requirement_types)
  );

  const prefixChanged = prefixConfig && (
    restrictPrefixes !== prefixConfig.restrict_prefixes ||
    JSON.stringify(allowedPrefixes) !== JSON.stringify(prefixConfig.allowed_prefixes)
  );

  const handleSaveIdConfig = () => {
    updateIdMutation.mutate({
      format,
      numbering,
      digits,
      requirement_types: reqTypes,
    });
  };

  const handleSavePrefixes = () => {
    updatePrefixMutation.mutate({
      restrict_prefixes: restrictPrefixes,
      allowed_prefixes: allowedPrefixes,
    });
  };

  const addReqType = () => {
    if (!newTypeName.trim() || !newTypePrefix.trim()) return;
    setReqTypes([...reqTypes, { name: newTypeName.trim(), prefix: newTypePrefix.trim().toUpperCase(), description: '' }]);
    setNewTypeName('');
    setNewTypePrefix('');
  };

  const removeReqType = (idx: number) => {
    setReqTypes(reqTypes.filter((_, i) => i !== idx));
  };

  const addPrefix = () => {
    if (!newPrefix.trim()) return;
    const p = newPrefix.trim().toUpperCase();
    if (!allowedPrefixes.includes(p)) {
      setAllowedPrefixes([...allowedPrefixes, p].sort());
    }
    setNewPrefix('');
  };

  return (
    <div className="flex flex-col gap-8 max-w-xl">
      {/* ID Configuration */}
      <section className="flex flex-col gap-4">
        <h3 className="text-sm font-semibold text-content">ID Configuration</h3>

        <div className="flex gap-3">
          <label className="flex flex-col gap-1 flex-1">
            <span className="text-xs font-medium text-content-secondary">Format</span>
            <select
              value={format}
              onChange={(e) => setFormat(e.target.value as IdFormat)}
              className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
            >
              <option value="SingleLevel">Single Level (PREFIX-NNN)</option>
              <option value="TwoLevel">Two Level (FEAT-TYPE-NNN)</option>
            </select>
          </label>

          <label className="flex flex-col gap-1 flex-1">
            <span className="text-xs font-medium text-content-secondary">Numbering</span>
            <select
              value={numbering}
              onChange={(e) => setNumbering(e.target.value as NumberingStrategy)}
              className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
            >
              <option value="Global">Global</option>
              <option value="PerPrefix">Per Prefix</option>
              <option value="PerFeatureType">Per Feature+Type</option>
            </select>
          </label>

          <label className="flex flex-col gap-1 w-24">
            <span className="text-xs font-medium text-content-secondary">Digits</span>
            <input
              type="number"
              min={1}
              max={6}
              value={digits}
              onChange={(e) => setDigits(Number(e.target.value))}
              className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content focus:border-accent focus:outline-none"
            />
          </label>
        </div>

        {/* Requirement types table */}
        <div className="flex flex-col gap-2">
          <span className="text-xs font-medium text-content-secondary">Requirement Types</span>
          <div className="rounded-lg border border-edge overflow-hidden">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-edge bg-surface-alt">
                  <th className="px-3 py-1.5 text-left text-xs font-medium text-content-secondary">Name</th>
                  <th className="px-3 py-1.5 text-left text-xs font-medium text-content-secondary">Prefix</th>
                  <th className="px-3 py-1.5 w-10"></th>
                </tr>
              </thead>
              <tbody>
                {reqTypes.map((rt, idx) => (
                  <tr key={idx} className="border-b border-edge last:border-b-0">
                    <td className="px-3 py-1.5 text-content">{rt.name}</td>
                    <td className="px-3 py-1.5 font-mono text-content-secondary">{rt.prefix}</td>
                    <td className="px-3 py-1.5">
                      <button
                        type="button"
                        onClick={() => removeReqType(idx)}
                        className="p-0.5 text-content-muted hover:text-red-400"
                      >
                        <X className="h-3.5 w-3.5" />
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className="flex gap-2 items-end">
            <input
              type="text"
              value={newTypeName}
              onChange={(e) => setNewTypeName(e.target.value)}
              className="flex-1 rounded-lg border border-edge bg-surface px-3 py-1.5 text-xs text-content focus:border-accent focus:outline-none"
              placeholder="Type name"
            />
            <input
              type="text"
              value={newTypePrefix}
              onChange={(e) => setNewTypePrefix(e.target.value.toUpperCase())}
              className="w-24 rounded-lg border border-edge bg-surface px-3 py-1.5 text-xs text-content font-mono focus:border-accent focus:outline-none"
              placeholder="PREFIX"
            />
            <button
              type="button"
              onClick={addReqType}
              className="flex items-center gap-1 text-xs text-accent hover:text-accent/80 shrink-0"
            >
              <Plus className="h-3.5 w-3.5" /> Add
            </button>
          </div>
        </div>

        {updateIdMutation.isError && (
          <p className="text-xs text-red-400">Failed to save ID config.</p>
        )}
        {updateIdMutation.isSuccess && (
          <p className="text-xs text-green-400">ID config saved.</p>
        )}

        <div>
          <button
            onClick={handleSaveIdConfig}
            disabled={!idChanged || updateIdMutation.isPending}
            className="rounded-lg bg-accent px-4 py-2 text-sm font-medium text-white hover:bg-accent/90 disabled:opacity-50 transition-colors"
          >
            {updateIdMutation.isPending ? 'Saving...' : 'Save ID Config'}
          </button>
        </div>
      </section>

      <hr className="border-edge" />

      {/* Prefix Configuration */}
      <section className="flex flex-col gap-4">
        <h3 className="text-sm font-semibold text-content">Prefix Management</h3>

        <label className="flex items-center gap-2 cursor-pointer">
          <input
            type="checkbox"
            checked={restrictPrefixes}
            onChange={(e) => setRestrictPrefixes(e.target.checked)}
            className="rounded border-edge accent-accent"
          />
          <span className="text-sm text-content">Restrict to allowed prefixes only</span>
        </label>

        <div className="flex flex-col gap-2">
          <span className="text-xs font-medium text-content-secondary">Allowed Prefixes</span>
          <div className="flex flex-wrap gap-1">
            {allowedPrefixes.map((p) => (
              <span key={p} className="inline-flex items-center gap-1 rounded-full bg-surface px-2.5 py-0.5 text-xs font-mono text-content border border-edge">
                {p}
                <button
                  type="button"
                  onClick={() => setAllowedPrefixes(allowedPrefixes.filter((x) => x !== p))}
                  className="text-content-muted hover:text-red-400"
                >
                  <X className="h-3 w-3" />
                </button>
              </span>
            ))}
          </div>
          <div className="flex gap-2">
            <input
              type="text"
              value={newPrefix}
              onChange={(e) => setNewPrefix(e.target.value.toUpperCase())}
              onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addPrefix(); } }}
              className="flex-1 rounded-lg border border-edge bg-surface px-3 py-1.5 text-xs text-content font-mono focus:border-accent focus:outline-none"
              placeholder="NEW_PREFIX"
            />
            <button
              type="button"
              onClick={addPrefix}
              className="flex items-center gap-1 text-xs text-accent hover:text-accent/80"
            >
              <Plus className="h-3.5 w-3.5" /> Add
            </button>
          </div>
        </div>

        {updatePrefixMutation.isError && (
          <p className="text-xs text-red-400">Failed to save prefix config.</p>
        )}
        {updatePrefixMutation.isSuccess && (
          <p className="text-xs text-green-400">Prefix config saved.</p>
        )}

        <div>
          <button
            onClick={handleSavePrefixes}
            disabled={!prefixChanged || updatePrefixMutation.isPending}
            className="rounded-lg bg-accent px-4 py-2 text-sm font-medium text-white hover:bg-accent/90 disabled:opacity-50 transition-colors"
          >
            {updatePrefixMutation.isPending ? 'Saving...' : 'Save Prefixes'}
          </button>
        </div>
      </section>
    </div>
  );
}
