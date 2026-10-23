# Product

<!-- impeccable:product-schema 1 -->

## Platform
Native desktop, macOS first; portable Rust architecture for future Linux support.

## Stack
User-selected Rust and GPUI. Build directly in GPUI and verify the running app.

## Users
Developers and power users working across directories, machines, object storage, and Git repositories.

## Product Purpose
Buckets is a keyboard-driven, multi-pane file browser. Users compare, find, preview, and manipulate files across independently navigable locations.

## Operating Context
Local drives, SSH hosts, S3-compatible buckets, and Git trees. Search integrates fd, rg, and fff when available. Existing user credentials and system tools provide authentication.

## Capabilities and Constraints
- All application workflows must be keyboard accessible; mouse, drag/drop and clipboard operations also work. Space opens Quick Look, matching Finder; x marks files.
- Application and plugin command shortcuts are configurable, with persistent overrides, conflict feedback and a way to restore defaults. Native text editing and raw terminal input retain their own keyboard behavior.
- Command-minus/plus adjusts saved text size across file browsing, Quick Look and embedded terminals; Command-0 resets it.
- Appearance supports independently configurable app/terminal font families and semantic color tokens, persisted in JSON and applied live. Tokyo Night and bundled JetBrains Mono remain the defaults, with explicit reset and unavailable-font recovery.
- Independent panes; safe rename, copy, move, duplicate, create, and delete workflows.
- Places combines saved local/remote locations and optional built-in shortcuts in one keyboard-accessible list; existing bookmark data and shortcut IDs remain compatible.
- File mutations share a bounded durable operation journal, with inspectable per-item outcomes, honest cancellation, safe retry and guarded local Undo. Uncertain effects never replay automatically; remote and cross-filesystem Undo remain unavailable.
- Per-pane sorting, optional metadata columns, adjustable proportions and horizontal/vertical arrangements persist without making everyday browsing a dashboard.
- Named workspaces preserve full provider locations, active pane and view geometry with separate note/agent identities. Restore never reruns terminal commands or model requests, and transitions protect dirty drafts and live terminals.
- A simple, versioned plugin API supports Rust and higher-level languages. Git status decorations, Neovim and Lazygit launching, and opening a terminal at the current directory must be plugins. These tools open embedded Alacritty-backed GPUI terminal panes by default; iTerm2 remains the selected external option.
- Search extensions configure indexing roots, gitignore behavior, embedding models and vector stores. The first-party AI plugin supplies a bounded on-demand embedding index and can delegate retrieval to another search plugin.
- A reusable TypeScript plugin SDK supports ordinary commands and persistent services. AI behavior belongs in a plugin, with pi-durable owning conversations and explicit workspace memory; Rust exposes bounded provider-aware workspace/read/search operations.
- Agent workflows include Ask these panes with file citations, semantic/hybrid results that remain navigable files, and inspectable/editable workspace notes. New AI setups default to Sign in with ChatGPT; API keys and local providers remain explicit alternatives. Existing provider selections are preserved. Model availability follows the selected account, and embeddings retain separate configuration. Missing credentials and plan limits have actionable recovery. File mutation is outside this first agent slice.
- macOS is the initial verification target. Other platforms are unverified until tested there.

## Brand Commitments
User requested Omarchy-inspired styling and developer joy. Working product name inferred from the folder: Buckets.

## Evidence on Hand
The repository contains the native application, renderer-independent core and
plugin host, embedded terminal, TypeScript SDK, first-party plugins, examples and
automated fixtures. [Verification](docs/verification.md) records implemented
behavior and evidence limits; ignored local captures/build outputs are not
prerequisites for a fresh clone. The application shows real filesystem data.

## Product Principles
- UI copy is functional: name actions, locations, state and recovery; no slogans or promotional language.
- Keep the keyboard path complete and discoverable.
- Prefer safe operations and explicit errors to hidden destructive behavior.
- Treat location providers and search integrations as extensible contracts.
- Keep slow work off the UI thread.

## Accessibility & Inclusion
Every application command is reachable by keyboard. Focus, selection, and active-pane state remain distinct and visible. Screen-reader support requires separate platform verification.
