# Logo

The Open Desktop mark and wordmark, drawn live so they follow the theme.

The mark is an equivalence sign. The top bar is cut unevenly, like x86-64 instructions of variable length; the bottom bar is cut into four equal words, like 32-bit ARM64 instructions; the middle bar in `trace` says they are the same program.

- Provide `size` (mark height in px). From 24 px up the full mark is drawn; below 24 px the component switches to the small mark, with two top segments, so the cuts stay visible.
- `lockup="horizontal"` (default) sets the wordmark in `display` weight 600 at 0.6 × size, with a gap of 0.25 × size. `lockup="mark"` is the mark alone.
- `tone="mono"` paints all three bars in the current text color. Use it on photos, on `trace` fills, in one-color print and embossing.
- The ink follows CSS `color`; the middle bar follows `trace`.

Do:
- Keep clear space of one bar height (5.5/48, about 1/9 of the mark) on every side.
- Use the mark alone where the name is already on screen: Dock, menu bar, favicon.

Don't:
- Recolor the top or bottom bar, reorder the bars, or even out the top cuts.
- Set the wordmark in any face but Literata, or squeeze it.
- Put the full-color mark on `trace`, `pass` or `fail` fills; use `tone="mono"`.
