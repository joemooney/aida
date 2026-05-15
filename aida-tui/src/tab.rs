//! Tab model — N hosted sessions, one focused (plan Fork 2:
//! session-as-tab). [`TabManager`] is generic over the tab payload so the
//! index arithmetic (`next` / `prev` / `switch_to` / `remove`) is unit-
//! testable without spawning real PTYs.
//!
//! trace:STORY-132 | ai:claude

use crate::pty::PtyHost;
use anyhow::{bail, Result};

/// Soft cap on concurrently hosted sessions (plan risk #6 — N Claude
/// children is N× CPU / tokens / API). Overridable via `[tui] max_tabs`.
pub const MAX_TABS: usize = 4;

/// One hosted Claude session.
///
/// STORY-5 (crash recovery) will extend this with the `role` / `worktree`
/// fields its `.aida/tui-state.json` `TabRecord` needs; STORY-132 carries
/// only what the supervisor itself reads.
pub struct SessionTab {
    /// Stable, monotonic tab id — assigned at creation and never reused.
    /// PTY events carry this id, so output routing survives the index
    /// shifts that `TabManager::remove` causes.
    pub id: usize,
    /// Claude conversation id minted by the TUI and passed to
    /// `aida queue work --session-id` (TASK-112 makes it resumable).
    pub session_id: String,
    /// The EPIC/STORY/… scope this session is working.
    pub scope: String,
    /// The PTY hosting the child.
    pub pty: PtyHost,
    /// Short label shown in the status strip.
    pub title: String,
}

/// An ordered set of tabs with exactly one focused (when non-empty).
///
/// Generic over `T`: the supervisor uses `TabManager<SessionTab>`; tests
/// use a trivial payload to exercise the focus arithmetic in isolation.
pub struct TabManager<T> {
    tabs: Vec<T>,
    focused: usize,
    max: usize,
}

impl<T> TabManager<T> {
    /// Construct an empty manager with the given soft cap (clamped to a
    /// minimum of 1).
    pub fn new(max: usize) -> Self {
        Self {
            tabs: Vec::new(),
            focused: 0,
            max: max.max(1),
        }
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Index of the focused tab. Meaningless when [`Self::is_empty`].
    pub fn focused_index(&self) -> usize {
        self.focused
    }

    pub fn focused(&self) -> Option<&T> {
        self.tabs.get(self.focused)
    }

    pub fn focused_mut(&mut self) -> Option<&mut T> {
        self.tabs.get_mut(self.focused)
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.tabs.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.tabs.iter_mut()
    }

    /// Append a tab and focus it. Errors when the soft cap is reached —
    /// the caller surfaces the cap to the user rather than silently
    /// over-spawning Claude children.
    pub fn add(&mut self, tab: T) -> Result<usize> {
        if self.tabs.len() >= self.max {
            bail!(
                "tab cap reached ({} hosted sessions max — see `[tui] max_tabs`)",
                self.max
            );
        }
        self.tabs.push(tab);
        self.focused = self.tabs.len() - 1;
        Ok(self.focused)
    }

    /// Remove tab `idx`, returning the payload. Focus shifts to a
    /// neighbour: a tab below the removed one keeps the same visual
    /// position; removing at/above focus clamps back into range.
    pub fn remove(&mut self, idx: usize) -> Option<T> {
        if idx >= self.tabs.len() {
            return None;
        }
        let removed = self.tabs.remove(idx);
        if self.tabs.is_empty() {
            self.focused = 0;
        } else if idx < self.focused {
            // A tab before the cursor vanished — shift the cursor down.
            self.focused -= 1;
        } else if self.focused >= self.tabs.len() {
            // Removed the focused (or a later) tab past the new end.
            self.focused = self.tabs.len() - 1;
        }
        Some(removed)
    }

    /// Focus an absolute tab index. Returns `false` (no-op) when out of
    /// range — the prefix `1`-`9` bindings clamp rather than panic.
    pub fn switch_to(&mut self, idx: usize) -> bool {
        if idx < self.tabs.len() {
            self.focused = idx;
            true
        } else {
            false
        }
    }

    /// Focus the next tab, wrapping past the end.
    pub fn next(&mut self) {
        if !self.tabs.is_empty() {
            self.focused = (self.focused + 1) % self.tabs.len();
        }
    }

    /// Focus the previous tab, wrapping past the start.
    pub fn prev(&mut self) {
        if !self.tabs.is_empty() {
            let n = self.tabs.len();
            self.focused = (self.focused + n - 1) % n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_manager_switch_wraps_and_clamps() {
        let mut tm: TabManager<&str> = TabManager::new(MAX_TABS);
        tm.add("a").unwrap();
        tm.add("b").unwrap();
        tm.add("c").unwrap();
        // `add` focuses the newest tab.
        assert_eq!(tm.focused_index(), 2);
        // `next` wraps past the end.
        tm.next();
        assert_eq!(tm.focused_index(), 0);
        // `prev` wraps past the start.
        tm.prev();
        assert_eq!(tm.focused_index(), 2);
        // `switch_to` clamps: an out-of-range index is rejected, focus held.
        assert!(!tm.switch_to(9));
        assert_eq!(tm.focused_index(), 2);
        assert!(tm.switch_to(1));
        assert_eq!(tm.focused_index(), 1);
    }

    #[test]
    fn tab_manager_remove_focused_refocuses_neighbor() {
        let mut tm: TabManager<&str> = TabManager::new(MAX_TABS);
        tm.add("a").unwrap();
        tm.add("b").unwrap();
        tm.add("c").unwrap();
        tm.switch_to(1); // focus "b"
        let removed = tm.remove(1).unwrap();
        assert_eq!(removed, "b");
        // Focus index 1 now holds the neighbour "c".
        assert_eq!(tm.len(), 2);
        assert_eq!(tm.focused(), Some(&"c"));
        // Removing the last (focused) tab clamps focus back into range.
        tm.switch_to(1); // focus "c" (now at index 1)
        tm.remove(1);
        assert_eq!(tm.focused(), Some(&"a"));
        assert_eq!(tm.focused_index(), 0);
    }

    #[test]
    fn tab_manager_add_past_cap_is_rejected() {
        let mut tm: TabManager<u32> = TabManager::new(MAX_TABS);
        for i in 0..MAX_TABS as u32 {
            tm.add(i).expect("under the cap");
        }
        // The (MAX_TABS + 1)-th tab is refused, not silently dropped.
        let err = tm.add(99).unwrap_err();
        assert!(err.to_string().contains("tab cap"));
        assert_eq!(tm.len(), MAX_TABS);
    }

    #[test]
    fn tab_manager_remove_before_focus_shifts_cursor() {
        let mut tm: TabManager<&str> = TabManager::new(MAX_TABS);
        tm.add("a").unwrap();
        tm.add("b").unwrap();
        tm.add("c").unwrap();
        tm.switch_to(2); // focus "c"
        tm.remove(0); // drop "a", before the cursor
                      // "c" is still focused, now at index 1.
        assert_eq!(tm.focused(), Some(&"c"));
        assert_eq!(tm.focused_index(), 1);
    }
}
