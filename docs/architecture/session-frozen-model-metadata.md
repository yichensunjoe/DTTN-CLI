# Session-frozen model metadata

DTTN freezes the non-secret runtime model contract when a session is created.
The snapshot contains the routing model, protocol, provider extensions, context
window, maximum output, and the current catalog capability/pricing evidence.

Snapshots are append-only JSON files under the session directory. Writes use a
temporary file followed by a same-directory rename, so readers on macOS and
Windows either observe a complete snapshot or fall back to the previous valid
generation. API keys, request headers, cookies, bearer tokens and full provider
URLs are never persisted.

On resume, the newest valid snapshot is restored before Chat State, the sampler
actor and compaction policy are initialized. Provider metadata refreshes may
update the shared model catalog for future sessions, but cannot change limits or
protocol in the active session. An explicit model switch creates a new snapshot
generation and becomes the new resume point.

This is the authoritative input for the later status-runtime snapshot and cost
segments. Provider billing values remain live telemetry and are not part of the
frozen model contract.
