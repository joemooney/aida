import type { BurnupPoint } from '../../../lib/sprint-utils';

interface BurnupChartProps {
  data: BurnupPoint[];
}

const W = 400;
const H = 200;
const PAD = { top: 20, right: 20, bottom: 30, left: 36 };

export function BurnupChart({ data }: BurnupChartProps) {
  if (data.length < 2) {
    return <p className="text-xs text-content-muted italic">Not enough data for burn-up chart.</p>;
  }

  const maxY = Math.max(...data.map((d) => Math.max(d.completed, d.scope)));
  const chartW = W - PAD.left - PAD.right;
  const chartH = H - PAD.top - PAD.bottom;

  const x = (i: number) => PAD.left + (i / (data.length - 1)) * chartW;
  const y = (v: number) => PAD.top + chartH - (v / (maxY || 1)) * chartH;

  const scopePoints = data.map((d, i) => `${x(i)},${y(d.scope)}`).join(' ');
  const completedPoints = data.map((d, i) => `${x(i)},${y(d.completed)}`).join(' ');

  // Fill area under completed line
  const completedArea =
    `${x(0)},${y(0)} ` + data.map((d, i) => `${x(i)},${y(d.completed)}`).join(' ') + ` ${x(data.length - 1)},${y(0)}`;

  const labelIndices = [0, Math.floor(data.length / 2), data.length - 1];

  return (
    <div className="flex flex-col gap-1">
      <h3 className="text-xs font-semibold text-content-secondary">Burn-up</h3>
      <svg viewBox={`0 0 ${W} ${H}`} className="w-full" style={{ maxHeight: 200 }}>
        {/* Grid lines */}
        {[0, 0.25, 0.5, 0.75, 1].map((frac) => {
          const yPos = PAD.top + chartH * (1 - frac);
          return (
            <g key={frac}>
              <line x1={PAD.left} y1={yPos} x2={W - PAD.right} y2={yPos} stroke="currentColor" strokeOpacity={0.08} />
              <text x={PAD.left - 4} y={yPos + 3} textAnchor="end" className="fill-content-muted" fontSize={9}>
                {Math.round(maxY * frac)}
              </text>
            </g>
          );
        })}

        {/* Completed area fill */}
        <polygon points={completedArea} fill="#10b981" fillOpacity={0.1} />

        {/* Scope line */}
        <polyline points={scopePoints} fill="none" stroke="#f59e0b" strokeWidth={1.5} strokeDasharray="6 3" strokeOpacity={0.7} />

        {/* Completed line */}
        <polyline points={completedPoints} fill="none" stroke="#10b981" strokeWidth={2} />

        {/* Data dots */}
        {data.map((d, i) => (
          <circle key={i} cx={x(i)} cy={y(d.completed)} r={2.5} className="fill-emerald-500" />
        ))}

        {/* X-axis labels */}
        {labelIndices.map((i) => (
          <text key={i} x={x(i)} y={H - 4} textAnchor="middle" className="fill-content-muted" fontSize={9}>
            {data[i].date.slice(5)}
          </text>
        ))}
      </svg>
      <div className="flex gap-4 text-[10px] text-content-muted">
        <span className="flex items-center gap-1">
          <span className="inline-block w-4 h-0.5 bg-emerald-500 rounded" /> Completed
        </span>
        <span className="flex items-center gap-1">
          <span className="inline-block w-4 h-0.5 bg-amber-500 rounded border-dashed" style={{ borderTop: '1.5px dashed #f59e0b', height: 0 }} /> Scope
        </span>
      </div>
    </div>
  );
}
