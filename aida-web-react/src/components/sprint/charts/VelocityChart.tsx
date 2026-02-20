import type { VelocityPoint } from '../../../lib/sprint-utils';

interface VelocityChartProps {
  data: VelocityPoint[];
}

const W = 400;
const H = 200;
const PAD = { top: 20, right: 20, bottom: 30, left: 36 };

export function VelocityChart({ data }: VelocityChartProps) {
  if (data.length === 0) {
    return <p className="text-xs text-content-muted italic">No velocity data available.</p>;
  }

  const maxY = Math.max(...data.map((d) => d.points), 1);
  const avg = data.reduce((s, d) => s + d.points, 0) / data.length;
  const chartW = W - PAD.left - PAD.right;
  const chartH = H - PAD.top - PAD.bottom;
  const barGap = 4;
  const barWidth = Math.min(40, (chartW - barGap * (data.length - 1)) / data.length);
  const totalBarsWidth = data.length * barWidth + (data.length - 1) * barGap;
  const offsetX = PAD.left + (chartW - totalBarsWidth) / 2;

  const y = (v: number) => PAD.top + chartH - (v / maxY) * chartH;

  return (
    <div className="flex flex-col gap-1">
      <h3 className="text-xs font-semibold text-content-secondary">Velocity</h3>
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

        {/* Average velocity line */}
        <line
          x1={PAD.left}
          y1={y(avg)}
          x2={W - PAD.right}
          y2={y(avg)}
          stroke="#a855f7"
          strokeWidth={1.5}
          strokeDasharray="6 3"
          strokeOpacity={0.7}
        />

        {/* Bars */}
        {data.map((d, i) => {
          const bx = offsetX + i * (barWidth + barGap);
          const barH = (d.points / maxY) * chartH;
          const by = PAD.top + chartH - barH;
          return (
            <g key={i}>
              <rect x={bx} y={by} width={barWidth} height={barH} rx={3} fill="#8b5cf6" fillOpacity={0.8} />
              {/* Value on top */}
              {d.points > 0 && (
                <text x={bx + barWidth / 2} y={by - 4} textAnchor="middle" className="fill-content-secondary" fontSize={9} fontWeight={600}>
                  {d.points}
                </text>
              )}
              {/* Label below */}
              <text x={bx + barWidth / 2} y={H - 4} textAnchor="middle" className="fill-content-muted" fontSize={9}>
                {d.sprintLabel}
              </text>
            </g>
          );
        })}
      </svg>
      <div className="flex gap-4 text-[10px] text-content-muted">
        <span className="flex items-center gap-1">
          <span className="inline-block w-3 h-3 bg-violet-500 rounded-sm opacity-80" /> Completed
        </span>
        <span className="flex items-center gap-1">
          <span className="inline-block w-4 h-0.5 bg-purple-400 rounded" style={{ borderTop: '1.5px dashed #a855f7', height: 0 }} /> Avg ({Math.round(avg)})
        </span>
      </div>
    </div>
  );
}
