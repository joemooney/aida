import type { Field } from 'react-querybuilder';
import type { Requirement } from '@shared/types';
import { STATUS_ORDER, TYPE_CONFIG } from './constants';
import { getSprintAssignmentTarget } from './sprint-utils';

const textOperators = [
  { name: '=', label: '=' },
  { name: '!=', label: '!=' },
  { name: 'contains', label: 'contains' },
  { name: 'beginsWith', label: 'begins with' },
];

const selectOperators = [
  { name: '=', label: '=' },
  { name: '!=', label: '!=' },
];

const numberOperators = [
  { name: '=', label: '=' },
  { name: '!=', label: '!=' },
  { name: '>', label: '>' },
  { name: '<', label: '<' },
  { name: '>=', label: '>=' },
  { name: '<=', label: '<=' },
  { name: 'between', label: 'between' },
];

const dateOperators = [
  { name: '=', label: '=' },
  { name: '>', label: 'after' },
  { name: '<', label: 'before' },
  { name: '>=', label: 'on or after' },
  { name: '<=', label: 'on or before' },
  { name: 'between', label: 'between' },
];

const priorityValues = [
  { name: 'High', label: 'High' },
  { name: 'Medium', label: 'Medium' },
  { name: 'Low', label: 'Low' },
];

const statusValues = STATUS_ORDER.map((s) => ({ name: s, label: s }));

const typeValues = (Object.keys(TYPE_CONFIG) as (keyof typeof TYPE_CONFIG)[]).map((t) => ({
  name: t,
  label: TYPE_CONFIG[t].label,
}));

/** Build query field definitions, dynamically discovering owners/features/sprints/custom fields from data. */
export function buildQueryFields(requirements: Requirement[]): Field[] {
  const owners = new Set<string>();
  const features = new Set<string>();
  const sprintMap = new Map<string, string>(); // id -> label
  const customFieldKeys = new Set<string>();

  for (const req of requirements) {
    if (req.owner) owners.add(req.owner);
    if (req.feature) features.add(req.feature);

    // Discover sprints
    if (req.req_type === 'Sprint') {
      const num = req.custom_fields?.sprint_number;
      sprintMap.set(req.id, num ? `Sprint ${num}` : req.title);
    }

    // Discover custom field keys
    if (req.custom_fields) {
      for (const key of Object.keys(req.custom_fields)) {
        if (!['sprint_number', 'sprint_goal', 'start_date', 'end_date', 'planned_velocity'].includes(key)) {
          customFieldKeys.add(key);
        }
      }
    }
  }

  // Also discover sprint assignments from non-sprint items
  for (const req of requirements) {
    const sprintId = getSprintAssignmentTarget(req);
    if (sprintId && !sprintMap.has(sprintId)) {
      // Sprint might be in list - look it up
      const sprint = requirements.find((r) => r.id === sprintId || r.spec_id === sprintId);
      if (sprint) {
        const num = sprint.custom_fields?.sprint_number;
        sprintMap.set(sprint.id, num ? `Sprint ${num}` : sprint.title);
      }
    }
  }

  const ownerValues = [...owners].sort().map((o) => ({ name: o, label: o }));
  const featureValues = [...features].sort().map((f) => ({ name: f, label: f }));
  const sprintValues = [...sprintMap.entries()].map(([id, label]) => ({ name: id, label }));

  const fields: Field[] = [
    {
      name: 'spec_id',
      label: 'ID',
      operators: textOperators,
      inputType: 'text',
    },
    {
      name: 'title',
      label: 'Title',
      operators: textOperators,
      inputType: 'text',
    },
    {
      name: 'description',
      label: 'Description',
      operators: [{ name: 'contains', label: 'contains' }],
      inputType: 'text',
    },
    {
      name: 'status',
      label: 'Status',
      operators: selectOperators,
      valueEditorType: 'select',
      values: statusValues,
    },
    {
      name: 'priority',
      label: 'Priority',
      operators: selectOperators,
      valueEditorType: 'select',
      values: priorityValues,
    },
    {
      name: 'req_type',
      label: 'Type',
      operators: selectOperators,
      valueEditorType: 'select',
      values: typeValues,
    },
    {
      name: 'owner',
      label: 'Owner',
      operators: selectOperators,
      valueEditorType: ownerValues.length > 0 ? 'select' : 'text',
      values: ownerValues.length > 0 ? ownerValues : undefined,
    },
    {
      name: 'feature',
      label: 'Feature',
      operators: selectOperators,
      valueEditorType: featureValues.length > 0 ? 'select' : 'text',
      values: featureValues.length > 0 ? featureValues : undefined,
    },
    {
      name: 'tags',
      label: 'Tags',
      operators: [{ name: 'contains', label: 'contains' }],
      inputType: 'text',
    },
    {
      name: 'weight',
      label: 'Points',
      operators: numberOperators,
      inputType: 'number',
    },
    {
      name: 'created_at',
      label: 'Created',
      operators: dateOperators,
      inputType: 'date',
    },
    {
      name: 'modified_at',
      label: 'Modified',
      operators: dateOperators,
      inputType: 'date',
    },
    {
      name: 'archived',
      label: 'Archived',
      operators: [{ name: '=', label: '=' }],
      valueEditorType: 'select',
      values: [
        { name: 'true', label: 'Yes' },
        { name: 'false', label: 'No' },
      ],
    },
  ];

  // Add sprint field if sprints exist
  if (sprintValues.length > 0) {
    fields.push({
      name: '_sprint',
      label: 'Sprint',
      operators: selectOperators,
      valueEditorType: 'select',
      values: sprintValues,
    });
  }

  // Add discovered custom fields
  for (const key of [...customFieldKeys].sort()) {
    fields.push({
      name: `_cf_${key}`,
      label: `Custom: ${key}`,
      operators: [
        { name: '=', label: '=' },
        { name: '!=', label: '!=' },
        { name: 'contains', label: 'contains' },
      ],
      inputType: 'text',
    });
  }

  return fields;
}
