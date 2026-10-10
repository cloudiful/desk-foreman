# Execution environment private service

An application reaches an execution environment's private agent over one
authenticated gateway. The gateway never dials the agent directly: it streams
the request through a reverse tunnel held by the environment's originally
bound runner manager.

```
application ──HTTPS──▶ control plane ──WebSocket──▶ runner manager ──HTTP──▶ private agent
                         │
                         └─ owner-scoped fence, bounded request, streamed response
```

## Endpoints

### `GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS /api/internal/execution-environments/{environment_id}/service/{path}`

Relays one request to the environment's private agent.

Required:

- `Authorization: Bearer <application token>` — the same application bearer
  the other internal routes use. It is the only identity on this path.
- `X-Execution-Environment-Generation: <int64>` — the caller's expected
  generation fence. It is a fence, not an identity: it must equal both the
  stored authorized generation and the provisioned container generation.
- `Content-Length` — required on every request. A relay promises a byte-exact
  body to the agent side, and cannot do that from an inferred length.

The caller never names a target. There is no manager id, host, or port
parameter anywhere on this path, and the query string is opaque text that is
never parsed as an address.

| Status | Meaning |
| --- | --- |
| 200 | Relayed agent status and streamed body. Server-sent events and binary bodies are relayed incrementally. |
| 400 | Unsupported method, unparseable path/query/header, or a missing `Content-Length`. |
| 401 | No application bearer. |
| 404 | A foreign or unknown environment, or an agent resource that does not exist. A foreign environment is indistinguishable from an unknown one. |
| 409 | Fence mismatch, environment not ready, or the relay cannot admit the stream. |
| 413 | Declared body length over the relay bound. |
| 503 | No live manager tunnel, or the tunnel ended the stream. |
| 504 | The agent service timed out. |

### `GET /api/internal/runner-manager/service-relay`

The manager's tunnel ingress.

- Authenticated with the existing runner-manager bearer.
- Requires the `df-env-http.v2` WebSocket subprotocol; the previous tag is refused.
- One live tunnel per manager: a second connection is refused with 409, and a
  late-disconnecting connection cannot remove the tunnel that replaced it.
- The control plane probes the tunnel every 15s and closes it after 30s of
  silence, failing whatever it was carrying.

## What crosses the relay

Request: the method, the bounded relative path, the opaque query, the exact
body, and only `Content-Type` and `Accept`. The caller's own authorization,
cookies, forwarding headers and hop headers are dropped and never reach the
agent.

Response: the agent's status, its streamed body, and only `Content-Type`,
`Cache-Control` and `Retry-After`. A relayed response can never set a cookie,
redirect the caller, or attach a credential to it.

## Bounds

| Limit | Value |
| --- | --- |
| Relative path | 1 KiB; no percent escapes, backslashes, control characters, empty/`.`/`..` segments, or authority shape |
| Raw query | 8 KiB, opaque |
| One forwarded header value | 2 KiB |
| Request or response body | 2 MiB |
| Control frame | 16 KiB |
| Transport message | 64 KiB |
| Data frame payload | 32 KiB |

## Admission

1. The application bearer is checked first.
2. The generation fence header is read and bounded.
3. Path, query, headers and body length are checked. Nothing else has been
   touched yet.
4. One owner-scoped row is read and both fences are checked against it. A
   foreign environment and an unknown one are both absent.
5. The stream is admitted to the manager's live tunnel.
6. The row is read *again*, and the stream's cancellation epoch is re-checked.
7. Only then is the opening frame written.

Steps 5 and 6 close the race where a stop or invalidation commits between the
caller's read and the tunnel write. The database is re-read rather than
trusted because the in-memory registry is a cache, not the lifecycle authority.

## Cancellation

A committed `stop` or `invalidate` cancels the environment's live streams. A
refused one changes nothing and leaves them running. Cancellation also
advances the environment's epoch, so a registration that races the mutation is
refused rather than admitted.

A client that disappears mid-body never has its request reported to the agent
as complete: the declared length and the bytes actually forwarded must agree
before the request is closed, and a short body is cancelled instead.

## Concurrency

Every control frame in both directions carries its own stream identifier
(`df-env-http.v2`): `open`, request bodies, `request_end`, `response_head`,
response data, `end`, `error` and `cancel` all correlate by reading the
frame, never by tunnel-singleton position. A tunnel multiplexes concurrent
streams — long-lived server-sent events alongside simultaneous POSTs — up to
8 per environment and 32 per manager; request bodies assemble in parallel
and responses serve on their own tasks. A reused identifier is refused with
an attributed fixed error, unknown or recently closed identifiers are
discarded without touching live streams, and a cancel removes only its named
stream. Quotas release exactly once however a stream ends.

## Manager side

The manager dials out; the control plane never dials in. With
`DESK_FOREMAN_URL` configured, the manager holds one authenticated tunnel
(`df-env-http.v2`, existing bearer header) beside the upstream pull worker,
reconnecting with bounded backoff and redacted logs. Without a URL or without
a Docker backend the relay stays disabled and startup is unaffected.

Request bodies assemble in parallel by identifier: under the environment mutex and
file reservation the manager re-checks the durable barrier (a stopped
generation never dials), the owner labels, and the running state, derives the
private address from `NetworkSettings.Networks[<trusted network>]` (private
IPv4 or unique-local IPv6 only; the daemon default address never counts),
reads the per-generation credential, and registers a sticky cancel receiver.
Both locks release before any HTTP, so a slow agent never wedges ensure or
stop, and nothing is ever auto-ensured or started for a relay.

The agent hop carries only `Content-Type`/`Accept` plus Basic auth from the
trusted username and the stored token. The client never redirects and never
consults environment proxies. The response head has a 10s deadline, a stream
lives at most 1h/128MiB, and bodies stream incrementally (SSE included)
without ever buffering a whole response. A committed `stop` cancels the
environment's live streams after persisting the stopped generation, including
the already-absent branch; a cancelled stream reports its fixed code rather
than a synthetic success, and no diagnostic or token text ever reaches the
wire.

## Related

- [Runner execution environment](./execution-environment-runner.md) — the
  trusted process profile and same-container state probe.
