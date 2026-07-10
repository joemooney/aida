-- Let long inline-code tokens break so they don't overflow narrow table cells.
-- trace:TASK-1129 | ai:claude
--
-- pandoc renders inline code as \texttt{...}, which LaTeX will not break inside.
-- A token like `max_threads/max_depth` or `.codex/agents/*.toml` is then wider
-- than its (narrow, in a table) column and spills into the next column, causing
-- the overlapping text seen in the comparison tables. This filter splits each
-- inline-code element at break-friendly boundaries (after / _ . - and at
-- camelCase seams) and inserts \allowbreak between the pieces, so xelatex may
-- wrap there when it has to. \allowbreak only *permits* a break — code that
-- fits on a line is unaffected. Each fragment is still emitted as normal
-- inline code, so pandoc handles all the LaTeX escaping.

function Code(el)
  local t = el.text
  local n = #t
  if n < 6 then return nil end -- short tokens never overflow; leave them alone

  local out, buf = {}, ""
  for i = 1, n do
    local c = t:sub(i, i)
    buf = buf .. c
    local nxt = t:sub(i + 1, i + 1)
    local brk = (c == "/" or c == "_" or c == "." or c == "-")
    -- camelCase / letter→digit-boundary seam: lower/digit followed by uppercase
    if nxt ~= "" and c:match("[%l%d]") and nxt:match("%u") then brk = true end
    if brk and i < n then
      table.insert(out, pandoc.Code(buf))
      table.insert(out, pandoc.RawInline("latex", "\\allowbreak{}"))
      buf = ""
    end
  end
  if #buf > 0 then table.insert(out, pandoc.Code(buf)) end

  if #out <= 1 then return nil end -- no break points found; unchanged
  return out
end
