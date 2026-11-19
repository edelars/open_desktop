# StageTable

The roadmap as a table: stage number, scope, status. Used in the README, on the site and in release notes.

- Provide `stages`: `{ n, scope, status, note? }` in roadmap order. `scope` is the roadmap's own wording; `note` narrows the status ("register-only profile").
- `caption` is an optional mono eyebrow above the table.
- Rows are light: scope in Literata 350, stage numbers in `code` weight 400 with tabular figures, status words at 500. Only the head and the caption use the bold `label` style.
- Rows are separated by `line`, never by shadow or zebra fills.

Keep it in sync with `docs/ROADMAP.md`. A stage changes status only when its acceptance report says so.
