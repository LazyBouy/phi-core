<!-- Last verified: 2026-04-05 by Claude Code -->
# WASM Plugin Architecture

> **Scope note:** WASM plugins are a baby-phi platform feature, not phi-core. This spec defines the design for future implementation in baby-phi.

The plugin system allows tools, callbacks, and filters to be defined as WebAssembly modules loaded at runtime. This removes the compile-time lock-in: users can extend the agent without recompiling the host binary.

**Status:** `[PLANNED]`

**Why:** In the current architecture, all tools and callbacks are Rust code compiled into the binary. For multi-user systems, each user's custom code requires a full recompile affecting all users. WASM plugins provide sandboxed, runtime-loadable extensions with language-agnostic authoring.

## Concept Overview

```
Plugin Architecture [PLANNED]
├── WIT Contracts [PLANNED] — typed interfaces for tool, callback, filter
│   ├── phi:tool/execute — tool execution contract
│   ├── phi:callback/* — lifecycle hook contracts (8 hook types)
│   └── phi:filter/input — input filter contract
├── Plugin Runtime [PLANNED] — wasmtime integration
│   ├── Loader — validate, instantiate, cache
│   ├── Resource Limits — memory, fuel, time
│   └── Host Capabilities — FS, network, env (scoped)
├── Plugin Manifest [PLANNED] — metadata (name, version, interfaces, capabilities)
└── Config Integration [PLANNED] — referenced in invocation config
```

---

## Plugin Contracts (WIT)

Contracts are defined using the [WebAssembly Interface Type (WIT)](https://component-model.bytecodealliance.org/design/wit.html) format. Each contract maps to an existing phi-core trait.

### `phi:tool/execute` — Tool Plugin

Maps to `AgentTool` trait. The primary plugin interface.

```wit
package phi:tool;

interface execute {
    record tool-params {
        arguments: string,       // JSON string of LLM-chosen arguments
        tool-call-id: string,
        tool-name: string,
    }

    record tool-result {
        content: string,         // JSON string of Vec<Content>
        is-error: bool,
        details: string,         // JSON string
    }

    // Tool metadata — called once at load time
    name: func() -> string;
    description: func() -> string;
    parameters-schema: func() -> string;  // JSON Schema string

    // Tool execution — called per invocation
    execute: func(params: tool-params) -> result<tool-result, string>;
}
```

**Host-side adapter:** `WasmToolAdapter` wraps a loaded plugin module and implements `AgentTool`. The agent loop sees it identically to a native Rust tool.

### `phi:callback/*` — Lifecycle Hook Plugins

Maps to the 8 callback types on `AgentLoopConfig`. Each hook is a separate WIT interface.

```wit
package phi:callback;

interface before-turn {
    // Args: JSON-encoded messages array, turn index
    // Returns: true to proceed, false to abort
    invoke: func(messages-json: string, turn-index: u32) -> bool;
}

interface after-turn {
    // Args: JSON-encoded messages array, JSON-encoded usage
    invoke: func(messages-json: string, usage-json: string);
}

interface before-compaction-start {
    // Args: loop_id, JSON messages, estimated_tokens, message_count
    // Returns: true to proceed, false to skip compaction
    invoke: func(loop-id: string, messages-json: string, estimated-tokens: u32, message-count: u32) -> bool;
}

interface after-compaction-end {
    invoke: func(loop-id: string, msgs-before: u32, msgs-after: u32, tokens-before: u32, tokens-after: u32, loops-compacted: u32);
}

// ... similar interfaces for before-loop, after-loop,
//     before-tool-execution, after-tool-execution, on-error
```

### `phi:filter/input` — Input Filter Plugin

Maps to `InputFilter` trait.

```wit
package phi:filter;

interface input {
    variant filter-result {
        pass,
        warn(string),
        reject(string),
    }

    filter: func(text: string) -> filter-result;
}
```

---

## Plugin Runtime

### Engine: wasmtime

**Why wasmtime over wasmer:** wasmtime is the reference implementation of the WebAssembly Component Model, maintained by the Bytecode Alliance (Mozilla, Fastly, Intel). It has the best WIT support, strongest security track record, and is the standard in the Rust ecosystem (used by Spin, Fermyon, Fastly Compute).

### Plugin Lifecycle

```
1. LOAD     — Read .wasm file from path or registry
2. VALIDATE — Verify WIT interface compliance + manifest check
3. COMPILE  — AOT compile to native code (cached for subsequent loads)
4. INSTANTIATE — Create module instance with host capabilities
5. EXECUTE  — Call interface functions (execute, invoke, filter)
6. UNLOAD   — Drop instance, reclaim resources
```

### Resource Limits

Per-plugin limits enforced by wasmtime:

| Limit | Default | Description |
|-------|---------|-------------|
| `max_memory_bytes` | 64 MB | WASM linear memory ceiling |
| `max_fuel` | 1,000,000 | Instruction fuel (prevents infinite loops) |
| `max_execution_ms` | 30,000 | Wall-clock timeout per call |
| `max_instances` | 10 | Concurrent instances of the same plugin |

### Host Capabilities

Plugins run in a sandbox. Host capabilities are explicitly granted per plugin:

| Capability | Default | Description |
|------------|---------|-------------|
| `fs_read` | scoped to workspace | Read files within the user's workspace |
| `fs_write` | scoped to workspace | Write files within the user's workspace |
| `network` | none | Network access: `none`, `allow_list`, `all` |
| `env_vars` | filtered | Access to env vars: `none`, `filtered_list`, `all` |
| `logging` | enabled | Emit log messages to host logger |
| `metrics` | enabled | Emit metric counters/histograms to host |

Capabilities are declared in the plugin manifest and granted (or denied) at load time.

---

## Plugin Manifest

Each plugin ships with a `plugin.toml` manifest:

```toml
[plugin]
name = "sql-query"
version = "1.0.0"
description = "Execute SQL queries against configured databases"
authors = ["user@example.com"]

[interfaces]
# Which WIT interfaces this plugin implements
tool = true
# callback_before_turn = true  (uncomment to implement callbacks)

[capabilities]
# What host capabilities this plugin requires
network = { allow_list = ["db.internal:5432"] }
fs_read = true
env_vars = { filtered_list = ["DATABASE_URL"] }
```

---

## Config Integration

Plugins are referenced in the invocation config (Phase 1):

```toml
# Tool plugin
[tools.plugins.sql_query]
path = "./plugins/sql-query.wasm"
# or from registry:
# registry = "phi-plugins://sql-query@1.0"

# Callback plugin
[callbacks]
before_turn = { plugin = "./plugins/guardrails.wasm", interface = "before-turn" }

# Filter plugin
[filters]
input = [{ plugin = "./plugins/pii-detector.wasm" }]
```

---

## Core vs External Boundary

| Component | Boundary | Rationale |
|-----------|----------|-----------|
| WIT contract definitions | **Core** | Defines the contract. All plugins implement same interfaces. |
| Plugin loader (wasmtime) | **Core** | Deep loop integration — plugins run in tool execution and callback paths. |
| Plugin manifest schema | **Core** | Standardizes plugin metadata. |
| Host capability definitions | **Core** | Security contract. Centrally controlled. |
| WasmToolAdapter | **Core** | Bridges plugin → `AgentTool` trait. |
| Plugin registry (download, discovery) | **External** | Infrastructure. Deployment-specific. |
| Plugin development SDK | **External** | Developer tooling. Separate crate. |
| Plugin signing / verification | **External** | Security infrastructure. Deployment-specific trust model. |

---

## Code Reference

| File | What it will contain |
|------|---------------------|
| `wit/phi-tool.wit` | Tool plugin WIT contract |
| `wit/phi-callback.wit` | Callback plugin WIT contracts |
| `wit/phi-filter.wit` | Filter plugin WIT contract |
| `src/plugins/runtime.rs` | wasmtime engine, loader, resource limits |
| `src/plugins/tool_adapter.rs` | `WasmToolAdapter` — wraps plugin as `AgentTool` |
| `src/plugins/callback_adapter.rs` | Wraps plugin callbacks as `BeforeTurnFn` etc. |
| `src/plugins/filter_adapter.rs` | Wraps plugin filters as `InputFilter` |
| `src/plugins/manifest.rs` | Plugin manifest parser |
| `src/plugins/mod.rs` | Module root + re-exports |

---

## Design Decisions

- **WIT over custom protocol:** WIT is the standard for WebAssembly Component Model. It provides typed interfaces, automatic bindings generation (via `wit-bindgen`), and cross-language support. A custom protocol would require maintaining codegen tooling.
- **wasmtime over wasmer:** wasmtime is the Bytecode Alliance reference runtime with the best WIT/Component Model support. wasmer has faster cold-start but weaker component model integration.
- **JSON serialization for complex types:** Messages and tool arguments are passed as JSON strings across the WASM boundary. This avoids the complexity of mapping phi-core's full type system into WIT records while maintaining backward compatibility. Future optimization: native WIT records for hot-path types.
- **Sync execution:** Plugin calls are synchronous from the WASM side (the plugin's `execute` function blocks). The host wraps them in `tokio::task::spawn_blocking` to avoid blocking the async runtime. Async WASM (via WASI preview 2) is a future enhancement.
- **Capability-based security:** Plugins declare required capabilities; the host grants or denies at load time. This follows the principle of least privilege. A plugin that only needs to read files never gets network access.
