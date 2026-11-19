# Open Desktop brand

Source of truth for the brand: mark, color and type tokens, components, voice and trademark rules. The live, browsable copy is the "Open Desktop" design system artifact; keep both in sync.

- [VOICE.md](VOICE.md): voice and writing rules.
- [TRADEMARK.md](TRADEMARK.md): how the name and the mark may be used (draft, needs legal review).
- [tokens.json](tokens.json): color tokens for light and dark themes, type scale, spacing, radii.
- [assets/](assets/): mark and app icon SVG masters.
- [components/](components/): reference React components (`window.OpenDesktop`) and their previews.

Open Desktop is an emulation engine, written from scratch, for running x86-64 Windows and Linux on Apple Silicon Macs. The brand reads like a lab protocol: every claim comes with its evidence, every status is stated exactly, and the mark itself is an equation, x86-64 ≡ ARM64.

Three principles drive every choice below:

- **Correctness first.** Show the check, not the adjective. A number appears with how it was measured.
- **Honest status.** Use the roadmap's status words as they are. Nothing is "almost done".
- **Native on the Mac.** The app looks like macOS; the brand lives in the mark, the icon, the site and the documents.

## Mark

The mark is an equivalence sign built from three bars of equal height.

- Top bar, `ink`: four segments of uneven length, the bytes of variable-length x86-64 instructions.
- Middle bar, `trace`: one solid bar, the equivalence the project proves.
- Bottom bar, `ink`: four equal segments, 32-bit ARM64 instruction words.

Use the `Logo` component in code and the SVG files in [assets/](assets/) everywhere else.

- From 24 px up use `od-mark.svg` (light grounds) or `od-mark-reverse.svg` (dark grounds).
- Below 24 px use `od-mark-small.svg`: two top segments instead of four, drawn on a 16 px grid so the cuts stay visible in the menu bar and in a favicon.
- On photos, on `trace` fills and in one-color print use `od-mark-mono.svg`.
- Clear space is one bar height (5.5/48, about 1/9 of the mark's height) on every side. Minimum size of the full mark is 24 px; of the horizontal lockup, a 20 px mark.
- The horizontal lockup sets "Open Desktop" in Literata 600 at 0.6 of the mark height, 0.25 of the mark height away from it. Use the vertical stack only when a square is the only space available: mark on top, name centered below at the same proportions.

Never even out the top cuts, swap the bar colors, rotate the mark, add outlines, shadows or gradients, or put the full-color mark on a `pass`, `fail` or `trace` fill.

## Color

The palette is ink on grid paper with one blue trace. Status colors are a separate set and mean only status.

- Page ground is `grid`; tables, cards and the VM row sit on `surface-raised`.
- Draw the protocol grid with `grid-line` at `grid-pitch` (16 px) on hero areas, covers and empty states. Never under running text longer than two lines.
- Set text in `ink`, secondary text in `ink-muted`. Both read on `grid` and `surface-raised` in light and dark themes.
- `trace` is the brand's single accent: the middle bar of the mark, links, the primary button, focus, selected rows (`trace-tint`). Use it for one thing per view.
- Text on a `trace` fill is `on-trace`, never a literal white: in the dark theme `trace` is light and `on-trace` is dark.
- `pass`, `fail` and `wip` with their `-tint` fills are for status only: `Status`, test output, error rows. Never use them for decoration, illustrations or marketing.
- Hairlines are `line`; borders of controls are `line-strong`, which holds 3:1 on both surfaces.
- Keyboard focus is a solid 2 px `focus-ring` with a 2 px offset on every surface.

## Type

- Headlines in Literata 600: `display` once per page, then `title` and `heading`. Literata is a book face with optical sizes; set it at the style sizes and let the browser pick the optical size.
- Running text on the site and in documents in Literata 400: `body`, and `body-small` for notes and measurement conditions.
- Status words, eyebrows and table heads in `label`: Spline Sans Mono 600, capitals, 0.08em tracking.
- Code, paths, crate names and register values in `code`; numbers in tables and measurements in `data` with tabular figures.
- The macOS app uses the system face through `ui` and `ui-strong`. Literata appears in the app only in the About window and the welcome screen.
- Spline Sans Mono has no Cyrillic. Russian text in mono roles falls back to SF Mono and Menlo; that is expected.

## Layout and surfaces

- Lay out on a 4 px base with the 16 px grid as the main step: `space-4` is one cell. Larger gaps are `space-6`, `space-8`, `space-12`.
- Separate with lines, not shadows. Tables have `line` between rows and no zebra fills. `shadow-overlay` is only for menus and popovers.
- Corners are small: `radius-1` for badges and inputs, `radius-2` for buttons and cards, `radius-0` for tables and bands. `radius-icon` belongs to the app icon only.
- No gradients anywhere in the brand.

## App icon

`od-app-icon.svg` follows the macOS icon grid: an 824 px body on a 1024 px canvas, corner radius `radius-icon`, `grid` fill with the protocol grid in `ink` at 7% and the full-color mark at 672 px (its bars span 448 px, more than half the body). Export 16, 32, 64, 128, 256, 512 and 1024 px from it; at 16 and 32 px replace the mark with `od-mark-small.svg`.

## Iconography

The brand defines no icon set of its own. In the macOS app use SF Symbols at the system weights. On the site and in documents use the status glyphs ≡ ≢ ≈ ○ (see `Status`) and plain text arrows (→) for flows such as x86-64 → ARM64. No emoji, no illustrations of chips or circuit boards.
