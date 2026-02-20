import { useState, useRef, useEffect } from 'react';
import { Pencil, Check, X } from 'lucide-react';
import { cn } from '../../lib/utils';

interface EditableTextProps {
  value: string;
  onSave: (value: string) => void;
  className?: string;
  inputClassName?: string;
  placeholder?: string;
  multiline?: boolean;
}

export function EditableText({ value, onSave, className, inputClassName, placeholder, multiline = false }: EditableTextProps) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value);
  const inputRef = useRef<HTMLInputElement | HTMLTextAreaElement>(null);

  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      // Put cursor at end
      if (inputRef.current) {
        const len = inputRef.current.value.length;
        inputRef.current.setSelectionRange(len, len);
      }
    }
  }, [editing]);

  // Sync draft when value changes externally
  useEffect(() => {
    if (!editing) setDraft(value);
  }, [value, editing]);

  function save() {
    const trimmed = draft.trim();
    if (trimmed && trimmed !== value) {
      onSave(trimmed);
    } else {
      setDraft(value);
    }
    setEditing(false);
  }

  function cancel() {
    setDraft(value);
    setEditing(false);
  }

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === 'Enter' && !multiline) {
      e.preventDefault();
      save();
    }
    if (e.key === 'Enter' && multiline && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      save();
    }
    if (e.key === 'Escape') {
      e.preventDefault();
      cancel();
    }
  }

  if (editing) {
    const sharedClasses = cn(
      'w-full rounded-lg border border-accent bg-surface px-3 py-1.5 text-sm text-content',
      'focus:outline-none focus:ring-1 focus:ring-accent',
      inputClassName,
    );

    return (
      <div className="space-y-2">
        {multiline ? (
          <textarea
            ref={inputRef as React.RefObject<HTMLTextAreaElement>}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={handleKeyDown}
            onBlur={save}
            rows={6}
            className={cn(sharedClasses, 'resize-y min-h-[80px]')}
            placeholder={placeholder}
          />
        ) : (
          <input
            ref={inputRef as React.RefObject<HTMLInputElement>}
            type="text"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={handleKeyDown}
            onBlur={save}
            className={sharedClasses}
            placeholder={placeholder}
          />
        )}
        <div className="flex items-center gap-1.5">
          <button
            onMouseDown={(e) => { e.preventDefault(); save(); }}
            className="flex items-center gap-1 rounded-md bg-accent px-2 py-1 text-[11px] font-medium text-white hover:bg-accent-hover transition-colors cursor-pointer"
          >
            <Check className="h-3 w-3" /> Save
          </button>
          <button
            onMouseDown={(e) => { e.preventDefault(); cancel(); }}
            className="flex items-center gap-1 rounded-md bg-surface-hover px-2 py-1 text-[11px] font-medium text-content-secondary hover:text-content transition-colors cursor-pointer"
          >
            <X className="h-3 w-3" /> Cancel
          </button>
          {multiline && (
            <span className="text-[10px] text-content-muted ml-auto">Ctrl+Enter to save</span>
          )}
        </div>
      </div>
    );
  }

  return (
    <div
      onClick={() => setEditing(true)}
      className={cn(
        'group/edit relative cursor-pointer rounded-lg transition-colors',
        'hover:bg-surface-hover/50',
        '-mx-2 px-2 py-0.5',
        className,
      )}
      title="Click to edit"
    >
      {value || <span className="text-content-muted italic">{placeholder ?? 'Click to add...'}</span>}
      <Pencil className="absolute right-1.5 top-1/2 -translate-y-1/2 h-3 w-3 text-content-muted opacity-0 group-hover/edit:opacity-100 transition-opacity" />
    </div>
  );
}

interface EditableSelectProps<T extends string> {
  value: T;
  options: readonly T[];
  onSave: (value: T) => void;
  renderOption?: (value: T) => React.ReactNode;
  renderValue?: (value: T) => React.ReactNode;
  className?: string;
}

export function EditableSelect<T extends string>({ value, options, onSave, renderOption, renderValue, className }: EditableSelectProps<T>) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function handleClickOutside(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    if (open) {
      document.addEventListener('mousedown', handleClickOutside);
      return () => document.removeEventListener('mousedown', handleClickOutside);
    }
  }, [open]);

  return (
    <div ref={ref} className={cn('relative', className)}>
      <button
        onClick={() => setOpen(!open)}
        className="cursor-pointer hover:opacity-80 transition-opacity"
      >
        {renderValue ? renderValue(value) : value}
      </button>
      {open && (
        <div className="absolute top-full left-0 mt-1 z-20 rounded-lg border border-edge bg-surface-alt shadow-xl shadow-black/20 py-1 min-w-[140px] animate-fade-in">
          {options.map((opt) => (
            <button
              key={opt}
              onClick={() => { onSave(opt); setOpen(false); }}
              className={cn(
                'w-full px-3 py-1.5 text-left text-xs transition-colors cursor-pointer',
                opt === value
                  ? 'text-accent bg-accent/10 font-medium'
                  : 'text-content-secondary hover:text-content hover:bg-surface-hover',
              )}
            >
              {renderOption ? renderOption(opt) : opt}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
