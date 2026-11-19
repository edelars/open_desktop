# VMRow

One virtual machine in the app library: name, hardware, state and the next command.

- Provide `name` (the person's own name for the VM), `state` and, when known, `cpus`, `memory` and `arch` (default "x86-64").
- `state="error"` outlines the row in `fail` and shows `detail`: what stopped the guest and where, in plain words plus the machine fact (fault, RIP). Then offer the next step as an action, usually "Open log".
- `actions` takes one or two small `Button`s. Put the primary action last.
- The row is set in the system face (`ui`), with hardware in `code`.

Don't show progress or speed claims in the row. Measurements belong in the VM's detail view, with their conditions.
