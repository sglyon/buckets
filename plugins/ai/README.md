# Workspace AI plugin

A TypeScript plugin built on the Buckets SDK and **pi-durable 1.3.0**, pinned
behind this adapter. Rust owns provider access and current workspace state.
The plugin owns conversations, explicit workspace notes, and its embedding cache.

## Install

Node.js **22.19+** and npm are required. From the Buckets checkout:

```sh
scripts/install-ai-plugin
```

The installer builds both packages, vendors the SDK, installs production
libraries into a standalone plugin directory, and records an absolute Node
executable. The installed plugin does not depend on checkout symlinks. It refuses
to overwrite an existing plugin. Restart Buckets after installation. For an
isolated install, pass `--config-dir /absolute/test/config`.

No package download happens at application startup. Installing dependencies is
an explicit step; application/plugin updates do not silently replace your code.

## Commands

- **Ask these panes**: a read-only model uses Rust's workspace snapshot, list,
  read and search tools. Answers contain full-location citations, validated by
  the host, with optional navigable result files.
- **Semantic search these panes**: embeddings and keyword ranking over bounded
  text prefixes, combined with reciprocal rank fusion. This does not require a
  chat-model credential when embeddings use a separate/local endpoint.
- **Workspace memory**: inspect and explicitly replace or clear notes. Works
  offline without any model configuration. Notes are versioned in pi-durable;
  previous values are retained in the document (last 20 edits).
- **Reindex semantic search**: explicitly rebuild the selected index scope.

These are progress messages, not token streaming. Model answers are returned
once committed by the harness. Tools cannot mutate files or invoke arbitrary
shell commands. The trusted plugin executable still inherits OS permissions;
the host API boundary is not an operating-system sandbox. The trusted bundled
manifest grants `/`; the host still narrows each invocation to current pane
roots and explicitly configured index roots. This grant does not authorize
arbitrary host-tool paths. Review it before installing third-party code.

## Configure

Open **Settings → Plugins → Workspace AI** and choose **Continue with ChatGPT**.
`chatgpt` is the default provider. Sign-in opens the system browser and asks for
permission to use your eligible ChatGPT plan. The connected account and its
available models appear in Settings. Leave the model empty to use the first
available account model, or choose one explicitly. **Manage usage** opens ChatGPT
settings; reaching a plan limit never switches to paid API calls automatically.

Accounts are separate registrations, even when they share an email address. You
can add, select, reconnect, and disconnect them. The Rust host stores credentials
in a separate owner-only account vault with atomic writes and refresh locking;
tokens never enter settings JSON, pi-durable conversations, or progress messages.
Disconnect attempts to revoke the remote session and clears local credentials;
an unconfirmed revocation is reported. Account/provider changes start a fresh
model context. Explicit workspace memory remains local and inspectable.

API keys and local models remain available: select `openai`, `anthropic`, or
`compatible` (a local/proxy server using OpenAI chat completions). Existing
explicit provider settings are preserved. API providers require an actual model
ID. A compatible
provider requires `base_url`; clear `api_key_env` for a keyless local server.
The Pi SDK uses a nonsecret internal compatibility placeholder in that case;
the plugin removes Authorization before sending the HTTP request. No ambient
API key or placeholder bearer is sent to the keyless server.
URLs cannot contain credentials, query parameters or fragments.

```json
{
  "plugins": {
    "ai": {
      "config": {
        "provider": "compatible",
        "model": "your-tool-capable-model",
        "base_url": "http://127.0.0.1:11434/v1",
        "api_key_env": "",
        "embedding_endpoint": "http://127.0.0.1:11434/v1/embeddings",
        "embedding_model": "your-embedding-model",
        "embedding_api_key_env": "",
        "index_roots": [
          { "location": "/absolute/project", "respect_gitignore": true }
        ],
        "max_files": 128,
        "max_file_bytes": 16384,
        "search_limit": 30
      }
    }
  }
}
```

This is a configuration example, not a claim those model names are installed.
Use `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or your configured environment variable
for cloud credentials; never paste key values into JSON. The GUI must inherit
these variables when launched (for example by launching its executable from an
appropriately configured shell). Settings contains variable **names**, not keys.
No credentials are written into model instructions, memory, or plugin logs.
ChatGPT plan usage covers eligible Responses requests; embeddings retain their
separately configured local or API provider. Sign-in does not grant Buckets access
to your ChatGPT conversations.

## Index scope and limits

`index_roots` carries full provider locations and explicit ignore policy; omitting
it uses the current directory panes and respects ignore rules. Roots must also
be permitted by the Rust broker. Traversal goes through `files.search`, not a
second JavaScript filesystem walker. Therefore local/SSH ignore behavior comes
from the existing Rust backend. Unsupported provider searches fail explicitly;
this release does not claim S3/Git-revision semantic indexing.

The default cache is local SQLite in the workspace's private plugin-state
folder. It stores sampled text plus vectors, keyed by root/ignore policy,
embedding endpoint/model, prefix limit, full file location and content hash.
Each semantic search refreshes the bounded candidate set, re-reads prefixes,
reuses unchanged vectors, and removes stale/ignored entries in the completed
scope. It is on-demand indexing, not a background filesystem watcher.

Default limits are 128 files, 16 KiB per file, and 30 results; settings expose
bounded increases (1,000 files and 64 KiB prefixes maximum). Text is chunked with
overlap; each result shows its indexed line, snippet, similarity and keyword
contribution. Large files are prefix-only. Binary, empty or unreadable candidates
are skipped. Traversal caps and skipped/truncated counts are reported. Embedding
requests are sent in batches with a 30-second timeout; vectors must be finite,
nonzero and dimensionally consistent. The endpoint receives indexed content.

Optional `search_plugin` delegates hybrid retrieval to an installed search
plugin instead of this cache; that plugin owns its own model/store policy. The
existing search-plugin contract currently requires local roots.

## Durability and cancellation

One process owns each workspace's pi database. The Rust supervisor holds an OS
lock; a PID guard also protects direct command-line launches. A live owner is
never displaced. A retry with the same request ID and identical snapshot finds
its prior submission; changed snapshots require a new request. Per-admission
fingerprint receipts remain valid after subsequent questions and process
restarts, rather than tracking only the most recent question. Conversations
and explicit notes survive reopening. Closing the agent pane or pressing Cancel
records cancellation. The UI does not automatically restore pending requests
after application restart. An uncancelled interrupted service can resume when
the host retries the identical request ID and snapshot; raw service disconnect
checkpoints pending state and stops live model work. No work runs without the
process.

Explicit cancellation is different: Rust records a durable cancellation receipt
before stopping the process. On reopening, the plugin reads receipts before
resuming and aborts the matching run, so cancelled work is not silently replayed.
A new question supersedes any older pending question in this workspace. The
harness deduplicates admissions, not external side effects; this release exposes
read-only tools only.

## Development and verification

```sh
npm ci --ignore-scripts
npm test
npm run typecheck
```

Tests use a deterministic test-only provider through the **real pi harness**,
SQLite restart fixtures, an embedding HTTP fixture, and the actual SDK wire.
They do not use cloud credentials or claim live provider/service certification.
The plugin itself has no fake-provider configuration switch.
