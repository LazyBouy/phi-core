# Multi-User Sandboxing

The multi-user layer enables multiple users to run agents on the same host with isolated plugins, sessions, and resources. Each user's custom code runs in a sandbox that cannot access other users' data.

**Status:** `[PLANNED]`

**Depends on:** Phase 1 (Invocation Layer), Phase 2 (WASM Plugin Architecture)

**Why:** Without isolation, a single phi-core deployment can only serve one user safely. Multi-user support is required for shared infrastructure (team servers, SaaS deployments, hosted agent platforms).

## Concept Overview

```
Multi-User Sandboxing [PLANNED]
├── User Identity [PLANNED] — minimal model: user_id + workspace path
├── Per-User Plugin Registry [PLANNED] — user's own loaded plugins
├── Sandbox Configuration [PLANNED] — resource limits per user
│   ├── WASM limits (memory, fuel, time)
│   ├── File system scope (workspace only)
│   ├── Network policy
│   └── Token/cost budget per session
├── Isolation Guarantees [PLANNED] — cross-user boundary enforcement
├── Shared vs Private Plugins [PLANNED] — system-wide built-ins vs user-installed
├── Session Isolation [PLANNED] — per-user session storage
└── Per-User Event Routing [PLANNED] — events delivered to correct user
```

---

## User Identity Model

Minimal model — phi-core defines the identity shape, not the authentication mechanism.

| Field | Type | Description |
|-------|------|-------------|
| `user_id` | string | Unique user identifier. Opaque to phi-core — could be UUID, email, OAuth sub. |
| `workspace` | path | Root directory for this user's file system access. All FS operations scoped here. |
| `plugin_dir` | path | Directory containing this user's installed plugins. |

**Authentication is external.** phi-core receives a verified `UserId` from the host application. How the host verifies identity (OAuth, JWT, API key, SSO) is not phi-core's concern.

---

## Per-User Plugin Registry

Each user has an isolated plugin namespace:

```
System plugins (shared, read-only):
  bash, read_file, write_file, edit_file, list_files, search
  + any system-wide WASM plugins installed by admin

User plugins (per-user, isolated):
  Loaded from user's plugin_dir
  Can only access user's workspace + granted capabilities
  Cannot see or interact with other users' plugins
```

### Resolution Order

When a tool is referenced by name in a user's config:

```
1. Check user's private plugins → found? use it
2. Check system shared plugins → found? use it
3. Error: tool not found
```

User plugins shadow system plugins by name — a user can override a system tool with their own implementation.

---

## Sandbox Configuration

Per-user resource limits, set by the host administrator.

### `UserSandbox` Struct

```rust
pub struct UserSandbox {
    pub user_id: String,
    pub workspace: PathBuf,
    pub plugin_dir: PathBuf,
    pub limits: UserLimits,
}

pub struct UserLimits {
    /// Max WASM memory per plugin instance
    pub max_plugin_memory_bytes: usize,       // default: 64 MB
    /// Max instruction fuel per plugin call
    pub max_plugin_fuel: u64,                 // default: 1_000_000
    /// Max wall-clock time per plugin call
    pub max_plugin_execution_ms: u64,         // default: 30_000
    /// Max concurrent plugin instances for this user
    pub max_plugin_instances: usize,          // default: 10
    /// Max total tokens per session
    pub max_session_tokens: u64,              // default: 1_000_000
    /// Max dollar cost per session (None = unlimited)
    pub max_session_cost: Option<f64>,
    /// Max concurrent sessions for this user
    pub max_concurrent_sessions: usize,       // default: 5
    /// Network access policy for this user's plugins
    pub network_policy: NetworkPolicy,        // default: None
    /// File system access scope
    pub fs_scope: FsScope,                    // default: WorkspaceOnly
}

pub enum NetworkPolicy {
    None,
    AllowList(Vec<String>),  // host:port patterns
    All,
}

pub enum FsScope {
    WorkspaceOnly,
    WorkspaceAndTemp,
    Custom(Vec<PathBuf>),
}
```

---

## Isolation Guarantees

| Boundary | Enforcement | Mechanism |
|----------|-------------|-----------|
| **File system** | User can only access their workspace | WASM capability: `fs_read`/`fs_write` scoped to `workspace` path |
| **Plugin code** | User's plugins cannot call other users' plugins | Separate WASM instances per user, no shared memory |
| **Sessions** | User can only access their own sessions | Session storage keyed by `user_id` |
| **Events** | User only receives events from their own agents | Event routing by `user_id` on `AgentContext` |
| **Env vars** | User's plugins see only their allowed env vars | Capability-filtered env var access per plugin |
| **Network** | User's plugins follow their network policy | WASM capability grants per user's `NetworkPolicy` |
| **Resources** | User's plugins respect their limits | wasmtime fuel/memory limits per `UserLimits` |

### What is NOT isolated (shared resources):

- **LLM API calls** — all users share the same provider connections. Rate limiting is per-API-key (external).
- **System plugins** — built-in tools (bash, file, search) run as host code, not WASM. They're scoped by the agent's workspace, not by WASM sandbox.
- **CPU/memory** — process-level resources are shared. Per-user limits apply to WASM plugins only. Host-side tools share the process.

---

## Shared vs Private Plugins

| Category | Installed by | Visible to | Sandboxed? | Example |
|----------|-------------|------------|------------|---------|
| **Built-in tools** | Compiled into binary | All users | No (host code) | bash, read_file, search |
| **System WASM plugins** | Admin | All users | Yes | company-specific tools |
| **User WASM plugins** | User | Only that user | Yes | user's custom tools |

---

## Session Isolation

Sessions are stored in per-user directories:

```
{storage_root}/
├── users/
│   ├── {user_id_1}/
│   │   ├── sessions/
│   │   │   ├── {session_id_1}.json
│   │   │   └── {session_id_2}.json
│   │   └── plugins/
│   │       └── {plugin_name}.wasm
│   └── {user_id_2}/
│       ├── sessions/
│       └── plugins/
└── system/
    └── plugins/
        └── {shared_plugin}.wasm
```

The existing `save_session()` / `load_session()` functions gain a `user_id` parameter for path scoping.

---

## Per-User Event Routing

`AgentContext` gains a `user_id: Option<String>` field. Events emitted during a loop carry the user_id, enabling the host to route events to the correct user's connection (WebSocket, SSE, etc.).

```rust
// On AgentContext:
pub user_id: Option<String>,

// On AgentEvent::AgentStart:
pub user_id: Option<String>,
```

This is a **core** change — the event stream must carry user identity so the host can route without parsing event content.

---

## Core vs External Boundary

| Component | Boundary | Rationale |
|-----------|----------|-----------|
| `UserSandbox` struct | **Core** | Defines the contract for per-user limits |
| `UserLimits` struct | **Core** | Defines what limits exist |
| WASM resource limit enforcement | **Core** | Security. Must be in the runtime. |
| `user_id` on `AgentContext` + events | **Core** | Cross-cutting. Events must route to correct user. |
| Session storage scoping by user_id | **Core** | Isolation guarantee. |
| Authentication / authorization | **External** | Infrastructure. OAuth, JWT, API keys — opinionated. |
| User management (CRUD, roles) | **External** | Application-specific. |
| Audit logging storage | **External** | Infrastructure (which DB, retention policy). |
| Usage tracking / billing | **External** | Application-specific business logic. |
| Rate limiting | **External** | Infrastructure. Per-API-key or per-user — deployment choice. |

---

## Code Reference

| File | What it will contain |
|------|---------------------|
| `src/sandbox/mod.rs` | Module root + re-exports |
| `src/sandbox/user.rs` | `UserSandbox`, `UserLimits`, `NetworkPolicy`, `FsScope` |
| `src/sandbox/registry.rs` | Per-user plugin registry (private + shared resolution) |
| `src/sandbox/storage.rs` | User-scoped session storage paths |
| `src/types/context.rs` | `user_id: Option<String>` on `AgentContext` |
| `src/types/event.rs` | `user_id: Option<String>` on relevant `AgentEvent` variants |

---

## Design Decisions

- **Minimal user model:** phi-core defines `user_id` + `workspace` — nothing more. Authentication, roles, permissions, and user management are external. This keeps the core focused on isolation mechanics, not identity management.
- **WASM-level isolation only:** Plugins run in WASM sandboxes. Built-in tools (bash, file ops) run as host code and are scoped by workspace path, not by WASM. Process-level isolation (separate processes per user) is a deployment concern, not a core feature.
- **Event routing, not event filtering:** The core adds `user_id` to events. The host application decides how to deliver events to users (WebSocket, SSE, polling). The core does not implement connection management.
- **Storage is file-based:** Per-user session storage uses the existing file-based approach, scoped by user_id in the path. Database-backed storage is external.
