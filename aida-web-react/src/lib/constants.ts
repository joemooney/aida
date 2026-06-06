import type { RequirementStatus, RequirementPriority, RequirementType } from '@shared/types';

export const STATUS_ORDER: RequirementStatus[] = [
  'Draft', 'Approved', 'Planned', 'InProgress', 'NeedsAttention', 'Done', 'Completed', 'Rejected',
];

// STORY-86: Done sits between InProgress and Completed. Lime-400 reads
// as "almost there" — visually distinct from Completed's settled
// emerald without looking like a different lifecycle stage.
// STORY-332: NeedsAttention is the off-mainline punt/pause state —
// fuchsia mirrors the CLI's bold-magenta palette and reads as "stop,
// decide something here", distinct from every on-track stage.
export const STATUS_CONFIG: Record<RequirementStatus, { color: string; bg: string; dot: string; label: string }> = {
  Draft:          { color: 'text-gray-400',    bg: 'bg-gray-500/10',    dot: 'bg-gray-400',    label: 'Draft' },
  Approved:       { color: 'text-blue-400',    bg: 'bg-blue-500/10',    dot: 'bg-blue-400',    label: 'Approved' },
  Planned:        { color: 'text-violet-400',  bg: 'bg-violet-500/10',  dot: 'bg-violet-400',  label: 'Planned' },
  InProgress:     { color: 'text-amber-400',   bg: 'bg-amber-500/10',   dot: 'bg-amber-400',   label: 'In Progress' },
  NeedsAttention: { color: 'text-fuchsia-400', bg: 'bg-fuchsia-500/10', dot: 'bg-fuchsia-400', label: 'Needs Attention' },
  Done:           { color: 'text-lime-400',    bg: 'bg-lime-500/10',    dot: 'bg-lime-400',    label: 'Done' },
  Completed:      { color: 'text-emerald-400', bg: 'bg-emerald-500/10', dot: 'bg-emerald-400', label: 'Completed' },
  Rejected:       { color: 'text-red-400',     bg: 'bg-red-500/10',     dot: 'bg-red-400',     label: 'Rejected' },
};

export const PRIORITY_CONFIG: Record<RequirementPriority, { color: string; icon: string; label: string }> = {
  High:   { color: 'text-red-400',   icon: 'ArrowUp',   label: 'High' },
  Medium: { color: 'text-amber-400', icon: 'Minus',     label: 'Medium' },
  Low:    { color: 'text-gray-400',  icon: 'ArrowDown', label: 'Low' },
};

// TASK-225: every RequirementType owns its own Tailwind hue family, so no two
// type badges are ever the same color in the type dropdown, kanban cards, or
// filter legend. The six doc-layer types (Principle/Vision/Constraint/Decision/
// Term/Doc — EPIC-24 living docs) form a distinct "knowledge/narrative" band:
//   - Principle (sky)   foundational north-star — airy blue
//   - Vision (violet)   aspirational horizon
//   - Constraint (rose) a guardrail/limit — warm, distinct from Bug's red
//   - Decision (amber)  a settled, deliberate choice — gold
//   - Term (emerald)    glossary anchor — a distinct green, off the neutrals
//   - Doc (stone)       prose/paper — a warm neutral
// The three neutral families are reserved for the three structural types:
// Task (slate, cool), Folder (gray, pure), Doc (stone, warm) — maximally
// spread so they stay separable. Term was pulled off zinc to avoid blurring
// with that trio; Decision moved off emerald to keep its hue out of the
// Completed-status echo. These replace PR-21's stub placeholders.
export const TYPE_CONFIG: Record<RequirementType, { color: string; bg: string; label: string }> = {
  Functional:    { color: 'text-blue-300',    bg: 'bg-blue-500/10',    label: 'Functional' },
  NonFunctional: { color: 'text-purple-300',  bg: 'bg-purple-500/10',  label: 'Non-Functional' },
  System:        { color: 'text-cyan-300',    bg: 'bg-cyan-500/10',    label: 'System' },
  User:          { color: 'text-green-300',   bg: 'bg-green-500/10',   label: 'User' },
  ChangeRequest: { color: 'text-orange-300',  bg: 'bg-orange-500/10',  label: 'Change Request' },
  Bug:           { color: 'text-red-300',     bg: 'bg-red-500/10',     label: 'Bug' },
  Epic:          { color: 'text-indigo-300',  bg: 'bg-indigo-500/10',  label: 'Epic' },
  Story:         { color: 'text-teal-300',    bg: 'bg-teal-500/10',    label: 'Story' },
  Task:          { color: 'text-slate-300',   bg: 'bg-slate-500/10',   label: 'Task' },
  Spike:         { color: 'text-yellow-300',  bg: 'bg-yellow-500/10',  label: 'Spike' },
  Sprint:        { color: 'text-pink-300',    bg: 'bg-pink-500/10',    label: 'Sprint' },
  Folder:        { color: 'text-gray-300',    bg: 'bg-gray-500/10',    label: 'Folder' },
  Meta:          { color: 'text-fuchsia-300', bg: 'bg-fuchsia-500/10', label: 'Meta' },
  // trace:TASK-225 | ai:claude — doc-layer "knowledge/narrative" palette band
  Principle:     { color: 'text-sky-300',     bg: 'bg-sky-500/10',     label: 'Principle' },
  Vision:        { color: 'text-violet-300',  bg: 'bg-violet-500/10',  label: 'Vision' },
  Constraint:    { color: 'text-rose-300',    bg: 'bg-rose-500/10',    label: 'Constraint' },
  Decision:      { color: 'text-amber-300',   bg: 'bg-amber-500/10',   label: 'Decision' },
  Term:          { color: 'text-emerald-300', bg: 'bg-emerald-500/10', label: 'Term' },
  Doc:           { color: 'text-stone-300',   bg: 'bg-stone-500/10',   label: 'Doc' },
};

export const AVATAR_COLORS = [
  'bg-indigo-500', 'bg-emerald-500', 'bg-amber-500', 'bg-rose-500',
  'bg-cyan-500', 'bg-violet-500', 'bg-teal-500', 'bg-orange-500',
];
