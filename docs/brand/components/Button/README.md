# Button

A command in the app or on the site, labelled with the verb it performs.

- Provide `children`: a verb or verb + object in sentence case ("Start", "Open log", "Take snapshot"). No "OK", no "Submit".
- `variant="primary"` fills with `trace` and sets text in `on-trace`. One per view: the action the person most likely came for.
- `variant="secondary"` (default) is `surface-raised` with a `line-strong` border.
- `variant="quiet"` is text in `trace` for low-weight links inside panels.
- `size="sm"` (24 px) for table rows and toolbars; the default is 32 px, matching macOS push buttons.
- Focus shows a solid 2 px `focus-ring` with a 2 px offset.

Don't put a status word on a button, or use `pass` or `fail` colors for buttons. A destructive command keeps the secondary look and says exactly what it removes ("Delete snapshot").
