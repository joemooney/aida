# Kanban Keyboard State Machine Plan (2026-03-01)

## Goals
- Make Kanban keyboard behavior predictable and discoverable.
- Keep card context when opening/closing detail view.
- Separate navigation intent from move intent.

## State Model
- `BoardNavigation(selectedCardId)`
- `BoardMove(selectedCardId)`
- `DetailView(selectedCardId)`
- `DetailEditDescription(selectedCardId)`

Notes:
- `selectedCardId` is independent from DOM focus.
- Selection is persisted in session storage (`kanban.selectedCardId`).

## Initial State
- On first open:
  - Select top-leftmost card (first non-empty status column, first card).
- On subsequent open:
  - Restore previous `selectedCardId` if still visible in filtered set.
  - Else fallback to top-leftmost card.

## Keyboard Semantics

### Navigation Mode
- `ArrowUp/Down`: move selection within current column.
- `ArrowLeft/Right`: move selection to neighboring column (clamped row index).
- `Enter`: open details for selected card in view mode.
- `Enter` again (while details are open): switch to description edit mode.
- `Space`: switch to Move mode.

### Move Mode
- `ArrowUp/Down`: reorder card in-column.
- `ArrowLeft/Right`: move card across statuses.
- `Esc`: switch back to Navigation mode.
- `Space`: toggle back to Navigation mode.

### Detail Interaction
- `Esc` in description editor: exit description edit first.
- `Esc` again in detail panel: close detail panel.
- On detail close: focus returns to current `selectedCardId`.

## Visual Feedback
- Selected card: persistent blue ring highlight.
- Keyboard-focused card: focus-visible accent ring.
- Recently moved card: temporary accent pulse.
- Mode badge in Kanban header (`Navigation`/`Move`).
- Inline hint text describing mode and key bindings.

## Drag-and-Drop Alignment
- Drag/drop shares same selection feedback:
  - moved card highlighted
  - scrolled into view
  - selection updated to moved card

## Accessibility Notes
- Cards are keyboard focusable (`tabIndex=0`).
- `aria-keyshortcuts` advertises primary key bindings.
- `aria-label` includes card identity and keyboard hint.
