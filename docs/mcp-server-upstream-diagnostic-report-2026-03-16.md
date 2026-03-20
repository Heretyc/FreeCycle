# FreeCycle MCP Server Diagnostic Report

Date: 2026-03-16
Workspace: `C:\Users\Lexi\Dropbox\freecycle`
Intended audience: upstream Claude Code instance working on the MCP server codebase

## Executive summary

The reported symptom was "FreeCycle agent server unreachable: Connection to localhost failed in both HTTPS and HTTP modes" from the MCP tools.

That message is misleading on this machine.

The FreeCycle desktop server is actually running and listening on `0.0.0.0:7443` in TLS mode. The primary failure is in the MCP client transport layer:

1. `secureFetch()` does not implement the self-signed TLS acceptance that its header comment claims.
2. HTTPS requests to the live FreeCycle server fail certificate validation in Node `fetch()`.
3. The fallback HTTP probe then talks plaintext to a TLS socket and fails, which gets collapsed into the generic "failed in both HTTPS and HTTP modes" error.
4. Separately, most Ollama-backed tools are currently bypassing FreeCycle entirely because proxy routing is gated on `server.tls_fingerprint`, and the default local config does not populate that field.

The net effect is a split-brain MCP experience:

- `freecycle_status`, `freecycle_health`, and any direct FreeCycle API calls fail.
- `freecycle_list_models`, `freecycle_generate`, `freecycle_embed`, and `freecycle_benchmark` can still work by talking directly to Ollama over `http://localhost:11434`.

## What was verified live

### Process and port state

Verified on this machine:

- `freecycle.exe` is running from `C:\Users\Lexi\Dropbox\freecycle\target\debug\freecycle.exe`
- `ollama.exe` is running
- TCP port `7443` is listening on `0.0.0.0`

This rules out "server not started" and "nothing bound to the port."

### FreeCycle runtime config

`%APPDATA%\FreeCycle\config.toml` currently contains:

- `[agent_server]`
- `port = 7443`
- `bind_address = "0.0.0.0"`
- `compatibility_mode = false`

That means the Rust app is expected to serve TLS, not plaintext HTTP.

### Direct transport repro

These probes were run from the local machine:

1. Plain Node fetch to HTTPS:

```js
fetch('https://localhost:7443/health')
```

Result: `TypeError: fetch failed`

2. Same probe with TLS verification disabled:

```powershell
$env:NODE_TLS_REJECT_UNAUTHORIZED='0'
node -e "fetch('https://localhost:7443/health').then(async r => console.log(r.status, await r.text()))"
```

Result: HTTP 200 with body:

```json
{"ok":true,"message":"FreeCycle is running"}
```

3. Plain HTTP probe to the same port:

```js
fetch('http://localhost:7443/health')
```

Result: `TypeError: fetch failed`

4. PowerShell `Invoke-WebRequest` to plaintext:

```powershell
Invoke-WebRequest http://localhost:7443/health
```

Result: protocol violation / malformed response behavior consistent with speaking HTTP to a TLS listener.

Conclusion: the server is healthy over HTTPS, but the client is rejecting its self-signed certificate and then misreporting the fallback outcome.

## Code-level findings

### Finding 1: `secureFetch()` does not actually trust self-signed TLS certs

File: [mcp-server/src/secure-client.ts](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\secure-client.ts)

Relevant lines:

- [mcp-server/src/secure-client.ts:33](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\secure-client.ts#L33)
- [mcp-server/src/secure-client.ts:57](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\secure-client.ts#L57)
- [mcp-server/src/secure-client.ts:67](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\secure-client.ts#L67)

Problem:

- The file header says it supports self-signed certificate acceptance and TOFU fingerprint verification.
- The implementation just calls `fetch(url, init)` with no custom TLS agent/dispatcher.
- In Node, that still performs normal certificate validation and rejects an unknown self-signed cert.
- The `entry?: ServerEntry` parameter is currently unused in the actual request path.

Impact:

- Every secure-mode FreeCycle server with a self-signed cert is treated as unreachable unless the runtime disables TLS verification globally.
- The error path obscures the real reason.

### Finding 2: the fallback error masks the actual TLS failure

File: [mcp-server/src/secure-client.ts](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\secure-client.ts)

Relevant lines:

- [mcp-server/src/secure-client.ts:68](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\secure-client.ts#L68)
- [mcp-server/src/secure-client.ts:74](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\secure-client.ts#L74)
- [mcp-server/src/secure-client.ts:80](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\secure-client.ts#L80)

Problem:

- The HTTPS attempt throws for certificate validation.
- The fallback HTTP attempt then fails because the server is not plaintext.
- The original TLS error is discarded.
- The final error claims both HTTPS and HTTP failed generically.

Impact:

- Users and tools are pushed toward bad hypotheses like wrong host, dead process, or closed port.
- Debugging time increases because the actionable signal, certificate rejection, is lost.

### Finding 3: `extractServerFingerprint()` cannot work as written

File: [mcp-server/src/secure-client.ts](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\secure-client.ts)

Relevant lines:

- [mcp-server/src/secure-client.ts:85](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\secure-client.ts#L85)
- [mcp-server/src/secure-client.ts:90](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\secure-client.ts#L90)
- [mcp-server/src/secure-client.ts:92](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\secure-client.ts#L92)

Problem:

- It uses `createConnection` from `node:net`, not `node:tls`.
- It then calls `getPeerCertificate()` on that socket.
- A plain TCP socket does not expose TLS peer certificate APIs.

Impact:

- The advertised TOFU path appears unfinished or dead code.
- Even if routing were fixed, there is no working mechanism here to obtain and pin a TLS fingerprint.

### Finding 4: proxy routing is coupled to `tls_fingerprint` presence, not actual server mode

File: [mcp-server/src/ollama-client.ts](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\ollama-client.ts)

Relevant lines:

- [mcp-server/src/ollama-client.ts:142](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\ollama-client.ts#L142)
- [mcp-server/src/ollama-client.ts:148](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\ollama-client.ts#L148)
- [mcp-server/src/ollama-client.ts:154](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\ollama-client.ts#L154)

Problem:

- `resolveBase()` uses the FreeCycle proxy only if `resolvedServer.tls_fingerprint` is truthy.
- The local default config at [mcp-server/freecycle-mcp.config.json](C:\Users\Lexi\Dropbox\freecycle\mcp-server\freecycle-mcp.config.json) does not include `tls_fingerprint`.
- So the MCP server routes Ollama traffic directly to `http://localhost:11434`, even while FreeCycle itself is running in secure mode on `7443`.

Impact:

- Tool behavior becomes inconsistent.
- Benchmarks and generation can succeed even when the FreeCycle transport is broken, masking the integration bug.
- Task signaling and FreeCycle health do not reflect the same path as inference.

### Finding 5: `freecycle_add_server` likely cannot add secure servers reliably

File: [mcp-server/src/tools.ts](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\tools.ts)

Relevant lines:

- [mcp-server/src/tools.ts:910](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\tools.ts#L910)
- [mcp-server/src/tools.ts:926](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\tools.ts#L926)

Problem:

- `freecycle_add_server` first probes secure status via `fc.getStatus(baseUrl)`.
- Because the current secure client rejects the self-signed cert, this probe fails against a healthy secure server.
- The function then falls back to plaintext probing, which also fails against the TLS server.
- Only after "success" does it write placeholder `tls_fingerprint` and `identity_uuid`.

Impact:

- Onboarding secure FreeCycle servers is probably broken or at least much more brittle than intended.

## Rust server side assessment

File: [src/agent_server.rs](C:\Users\Lexi\Dropbox\freecycle\src\agent_server.rs)

Relevant lines:

- [src/agent_server.rs:438](C:\Users\Lexi\Dropbox\freecycle\src\agent_server.rs#L438)
- [src/agent_server.rs:450](C:\Users\Lexi\Dropbox\freecycle\src\agent_server.rs#L450)
- [src/agent_server.rs:478](C:\Users\Lexi\Dropbox\freecycle\src\agent_server.rs#L478)

Observed behavior matches the Rust implementation:

- `compatibility_mode = false` means TLS mode
- the server binds and serves with `axum_server::bind_rustls(...)`
- the live process accepted the request when Node TLS verification was disabled

Inference:

- The Rust server is not the primary fault for this incident.
- The current issue is almost entirely on the MCP client side.

## Why some tools still worked

The successful tools were not proof that FreeCycle transport was healthy.

They worked because:

1. `ensureLocalAvailability()` first checks Ollama health directly.
2. `ollama-client.resolveBase()` routes directly to Ollama unless `tls_fingerprint` is present.
3. The config currently has no `tls_fingerprint` for the localhost server.

So `freecycle_benchmark` and similar tools were benchmarking local Ollama directly, not a validated FreeCycle secure path.

Relevant files:

- [mcp-server/src/availability.ts](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\availability.ts)
- [mcp-server/src/ollama-client.ts](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\ollama-client.ts)

## Suggested upstream fixes

### Priority 1: implement real TLS handling in `secureFetch()`

Requirements:

- Accept self-signed certificates only through an explicit trust model, not global TLS disable.
- Preserve normal verification for public CA-signed endpoints.
- Support TOFU or pinned fingerprint comparison using `ServerEntry.tls_fingerprint`.

Practical implementation direction:

- Use `undici.Agent` or Node TLS options with a custom `dispatcher` for `fetch`.
- Use `connect: { rejectUnauthorized: false }` only long enough to inspect the presented certificate.
- Extract the peer certificate via a TLS socket or the underlying TLS connection, compute SHA-256 over `cert.raw`, then:
  - if no fingerprint is stored and policy allows TOFU, store it
  - if a fingerprint is stored, compare and reject on mismatch
- After acceptance, continue the request on the trusted channel

### Priority 2: preserve the original TLS error in fallback reporting

Instead of:

- swallowing the HTTPS error
- trying HTTP
- returning only "failed in both modes"

Return an error that includes both attempts, for example:

```text
HTTPS failed: self-signed certificate
HTTP fallback failed: protocol mismatch or connection failure
```

That would have made this issue obvious immediately.

### Priority 3: decouple proxy routing from `tls_fingerprint` presence

Current logic in [mcp-server/src/ollama-client.ts:148](C:\Users\Lexi\Dropbox\freecycle\mcp-server\src\ollama-client.ts#L148) treats "fingerprint exists" as "server is secure and proxy should be used."

That is too indirect.

Better options:

- Explicitly store server transport mode in config, or
- Determine routing from actual server config / successful probe result, or
- Default FreeCycle hosts on port `7443` to proxy mode unless compatibility mode is explicitly configured otherwise

At minimum, local secure-mode setups should not silently bypass FreeCycle just because the fingerprint is absent.

### Priority 4: either fix or remove `extractServerFingerprint()`

If fingerprinting is needed, switch to `node:tls` and use a real TLS handshake.

If not ready yet, remove the misleading helper and the header comment claims until the feature exists.

### Priority 5: add an end-to-end secure-mode test path

Needed test coverage:

- spin up a local HTTPS test server with a self-signed cert
- verify `secureFetch()` can connect without `NODE_TLS_REJECT_UNAUTHORIZED=0`
- verify the certificate fingerprint is captured and compared
- verify mismatch detection fails closed
- verify plaintext fallback only occurs when the target is actually plaintext
- verify `freecycle_add_server` can onboard a secure server

## Recommended regression tests

### MCP TypeScript tests

Add tests for:

1. self-signed secure server on localhost
2. secure server with mismatched pinned fingerprint
3. plaintext compatibility server
4. HTTPS cert rejection error surfacing
5. `ollama-client.resolveBase()` routing behavior when:
   - secure server exists but fingerprint missing
   - secure server exists and fingerprint present
   - compatibility/plaintext mode is configured

### Integration test expectation

A secure local setup should satisfy all of these at once:

- `freecycle_status` succeeds
- `freecycle_health` reports `freecycle_reachable: true`
- `freecycle_list_models` succeeds
- `freecycle_generate` succeeds
- `freecycle_start_task` and `freecycle_stop_task` succeed

Right now that full path is broken.

## Repro sequence for upstream

On a Windows machine with FreeCycle running in secure mode:

1. Ensure `%APPDATA%\FreeCycle\config.toml` contains:

```toml
[agent_server]
port = 7443
bind_address = "0.0.0.0"
compatibility_mode = false
```

2. Configure the MCP server with:

```json
{
  "servers": [
    {
      "host": "localhost",
      "port": 7443,
      "name": "Local",
      "approved": true
    }
  ],
  "ollama": {
    "host": "localhost",
    "port": 11434
  }
}
```

3. Call `freecycle_status`

Current result:

- reports FreeCycle unreachable

4. Run:

```powershell
$env:NODE_TLS_REJECT_UNAUTHORIZED='0'
node -e "fetch('https://localhost:7443/health').then(async r => console.log(r.status, await r.text()))"
```

Expected result:

- HTTP 200
- proves the server is alive and the failure is certificate handling

## Bottom line

The upstream issue is not "FreeCycle server unreachable."

The verified root cause is:

- secure-mode FreeCycle is up
- the MCP secure client rejects its self-signed cert
- the fallback error hides that fact
- inference tools still appear healthy because direct Ollama routing bypasses FreeCycle when `tls_fingerprint` is absent

That combination makes the current MCP behavior internally inconsistent and hard to debug.
