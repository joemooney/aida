// trace:STORY-1.A1 | ai:codex
test("typescript above", () => {
  expect(true).toBe(true);
});

it('typescript inside', () => {
  // trace:STORY-1.A2 | ai:codex
  expect(true).toBe(true);
});

test("typescript untraced", () => {
  expect(true).toBe(true);
});
