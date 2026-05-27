# wallet-rs

Agent-ready Rust CLI for the BudgetBakers Wallet REST API.

## Install

```sh
cargo build --release
```

The binary is named `wallet`.

## Authentication

Create an API token in the Wallet web app, then pass it with `--token` or set:

```sh
export WALLET_API_TOKEN=...
```

## Examples

```sh
wallet schema --output json
wallet list records --limit 10 --filter recordDate=gte.2026-01-01 --fields id,amount,recordDate --output json
wallet list accounts --output ndjson
wallet references categories --id 5c5c03e8-000a-8000-8000-000000000000 --output json
wallet delete records --id a1b2c3d4-e5f6-7890-abcd-ef1234567890 --dry-run --output json
```

Use raw requests for endpoints that do not yet have a typed command:

```sh
wallet request get /v1/api/records/by-id --param id=a1b2c3d4-e5f6-7890-abcd-ef1234567890 --output json
```

Mutating commands accept raw API-shaped JSON:

```sh
wallet create records --json-file records.json --dry-run --output json
wallet patch records --json '[{"id":"...","note":"Updated"}]' --strict --dry-run --output json
```

## Agent-facing behavior

- Use `--output json` or `--output ndjson` for machine-readable output.
- Use `wallet schema --output json` as the runtime contract.
- Use `--fields`, `--limit`, and `--output ndjson` to keep responses small.
- Use `--json-file` for complex payloads to avoid shell quoting problems.
- Use `--dry-run` before create, patch, delete, or mutating raw requests.

The CLI rejects control characters, encoded or query-polluted IDs, and raw request paths outside `/v1/api`.
