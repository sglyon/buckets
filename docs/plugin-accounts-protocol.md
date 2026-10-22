# Plugin accounts

Plugins opt into account management with `"capabilities":["accounts"]`.
The account service uses the same executable and bounded duplex JSON-RPC
transport as an agent service. Account management is separate from workspace
tools: it cannot read browsed files, and a model cannot start browser sign-in.
Existing one-shot plugins need no changes.

## Public interface

Rust starts `PluginRegistry::start_accounts(id)` and invokes `accounts.invoke`
with the usual `protocol_version`, effective `config` and `request` envelope:

```json
{"action":"status","account_id":"optional-saved-registration"}
```

Actions are `status`, `sign_in`, `sign_out`, `select` and `models`. `select`
requires an account ID. A sign-in without an ID adds a registration; supplying
an ID reconnects it. Account IDs identify registrations, not email addresses.

The result contains public presentation data only:

```json
{
  "accounts":[{"id":"registration-1","label":"Account label","connected":true,"plan_enabled":true}],
  "active_account_id":"registration-1",
  "models":[{"id":"model-slug","name":"Model display name"}],
  "plan_enabled":true,
  "using_plan":true,
  "sign_in_required":false,
  "first_connection":false,
  "sign_in_label":"Continue with ChatGPT",
  "plan_label":"Using ChatGPT plan",
  "usage_url":"https://chatgpt.com/settings/usage"
}
```

An optional `message` explains a result or recovery. `accounts.progress`
notifications contain `{ "message": "Waiting for browser sign-in…" }`.
The host bounds counts and text lengths, validates identifiers and URLs, and
discards stale UI completions. Credentials must never appear in this result,
progress, settings, conversation state or diagnostics.

## Host callbacks

The host constructs a `CredentialBroker` with its own credential root and the
manifest's plugin ID. Plugins choose a bounded record key, never a path or
another plugin's namespace.

| Method | Parameters | Result |
| --- | --- | --- |
| `credentials.read` | `{key}` | `{value?}` |
| `credentials.acquire` | `{key}` | `{lease,value?}` |
| `credentials.commit` | `{lease,value?}` | `{}` |
| `credentials.release` | `{lease}` | `{}` |
| `browser.open` | `{url}` | `{}` |

`acquire` holds a nonblocking OS file lock across the complete read, refresh
and replacement. Retry a busy lock with bounded delay and cancellation.
`commit` replaces the record atomically and releases the lease; a null value
deletes it, and an omitted value leaves it unchanged. `release` discards a
pending change. Dropping the broker releases its leases. The Unix store uses
owner-only directories/files, ownership checks and directory-relative
no-follow operations. It is a private local file store, not macOS Keychain.

Only account-capable plugins receive credential callbacks. Their agent service
may use them to resolve or renew request credentials. `browser.open` is allowed
only during an explicit `sign_in` request with network permission; agent/model
requests cannot invoke it. The desktop validates external URLs and opens them
through the native application. Cancellation and timeout stop the owned plugin
process group and release the host's resources.

These grants govern host dispatch. Installed executables still run with the
user's OS permissions; the protocol is not an operating-system sandbox.

## Workspace AI

The TypeScript AI plugin owns OpenAI-specific OAuth, identity validation,
registration, account models and plan-limit recovery. Rust and GPUI depend on
the generic account interface. OAuth uses the system browser, PKCE, a loopback
callback and verified ID tokens. Registrations and rotating tokens live in the
credential broker, separately from workspace databases. New setups choose
ChatGPT; explicit API-key and local-provider settings remain available.
Embeddings retain an independently configured endpoint.

The supported inference route and current restrictions are documented in
[OpenAI's Sign in with ChatGPT guide](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference).
No fallback to a different billing provider happens automatically.
