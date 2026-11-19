# Status

The status word of a stage, a fixture run or a test, with its glyph. The brand's honesty lives here.

- Provide `status`. Stage statuses come straight from the roadmap: `accepted`, `in-progress`, `partial`, `early`, `planned`. Test results: `pass`, `fail`.
- Glyphs follow the mark: ≡ means verified equal (accepted, pass), ≢ means a divergence (fail), ≈ means not verified yet (in progress, partial, early work), ○ means not started.
- The word is always shown. Color alone never carries the status, so it reads in grayscale and for color-blind readers.
- `children` may narrow the word ("Accepted, register-only"), never soften it.

Don't round up: a stage with a skeleton or a passing build is `in-progress`, not `accepted`. Don't invent statuses such as "Almost done" or "Beta-ready".
