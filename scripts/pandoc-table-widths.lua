-- Size table columns by their content instead of equal-splitting.
-- trace:TASK-1128 | ai:claude
--
-- pandoc's LaTeX writer gives every column of a *wide* pipe table an equal
-- share of the text width. For the comparison tables in docs/positioning/ that
-- wastes a full third on the short row-label column ("Layer" / "State" / ...),
-- squeezing the content columns into needless wrapping. This filter rebalances
-- an already-wide table so each column's width is proportional to the longest
-- line it holds (capped, with a floor) — the label column shrinks and the
-- content columns get the room back.
--
-- Conservative by design: it only touches tables pandoc already assigned
-- explicit widths to. Compact tables pandoc left at natural width (e.g. narrow
-- numeric tables) are returned untouched, so they stay tight.

local stringify = pandoc.utils.stringify

local CAP  = 42    -- cap a column's weight so one very long cell can't hog width
local MINW = 0.07  -- floor per column so nothing collapses to a sliver

function Table(tbl)
  local ncol = #tbl.colspecs
  if ncol < 2 then return nil end

  -- Only rebalance tables pandoc already made "wide" (explicit width on col 1).
  local w1 = tbl.colspecs[1][2]
  if w1 == nil or w1 == 0 then return nil end

  local natural = {}
  for i = 1, ncol do natural[i] = 1 end

  local function scan(row)
    for i, cell in ipairs(row.cells) do
      local s = stringify(cell.contents or cell)
      local len = 0
      for line in (s .. "\n"):gmatch("(.-)\n") do
        if #line > len then len = #line end
      end
      if len > natural[i] then natural[i] = len end
    end
  end

  for _, r in ipairs(tbl.head.rows) do scan(r) end
  for _, b in ipairs(tbl.bodies) do
    for _, r in ipairs(b.body) do scan(r) end
  end

  local capped, total = {}, 0
  for i = 1, ncol do
    capped[i] = math.min(natural[i], CAP)
    total = total + capped[i]
  end
  if total == 0 then return nil end

  local widths, sum = {}, 0
  for i = 1, ncol do
    widths[i] = math.max(capped[i] / total, MINW)
    sum = sum + widths[i]
  end
  for i = 1, ncol do
    tbl.colspecs[i] = { tbl.colspecs[i][1], widths[i] / sum }
  end
  return tbl
end
