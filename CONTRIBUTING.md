# Contributing

Keep changes focused on the current phase in `pathpilot_project_plan.md`.

Before submitting a change, run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The architectural boundaries are intentional:

- `pathpilot-core` contains domain logic and must not depend on GTK.
- `pathpilot-ui-gtk` translates domain state and commands into GTK presentation.
- `pathpilot` composes and starts the application.

Do not perform filesystem or other blocking work on the GTK main thread.
