# Agent Notes

Use `wallet schema --output json` before invoking commands that need exact request or response shape.

Prefer these defaults:

- `--output json` for single responses.
- `--output ndjson` for large lists.
- `--fields` with only the fields needed for the next step.
- `--json-file` for create, patch, and raw request bodies.
- `--dry-run` before every create, patch, delete, or mutating `request`.

Do not pre-encode query values or IDs. Pass plain values and let the CLI and HTTP client encode the request.
