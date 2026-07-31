# ADR-0016: Use quick-xml for S3 ListObjectsV2 Response Parsing

## Status
Accepted

## Context
`S3Poller` (`crates/radar-workstation/src/ingest/s3_poll.rs`) discovers new NEXRAD
chunks by calling S3's `ListObjectsV2` API, which returns an XML document (`Key`,
`IsTruncated`, `NextContinuationToken`, and other fields this project ignores). XML has
enough edge cases — entity escaping, CDATA sections, encoding declarations, namespace
prefixes — that hand-parsing it correctly is the one place in the acquisition stack
where the ADR-0014 ownership-cost tradeoff runs the other way from the HTTP/1.1 client
itself: HTTP/1.1 framing is a small, well-specified, security-critical surface worth
owning; general XML parsing is a large surface with a poor cost/benefit ratio to
re-derive.

`quick-xml` was added before this ADR existed, flagged by the same dependency audit
that produced ADR-0014, ADR-0015, and this ADR. This document closes that gap.

## Decision
The `quick-xml` crate (`quick-xml = "0.41"`) is used for `ListObjectsV2` response
parsing, via its pull-based `Reader` / `Event` API operating directly on `&[u8]`.

`parse_list_xml` in `s3_poll.rs` reads only three tags it cares about (`Key`,
`IsTruncated`, `NextContinuationToken`), tracked with a small enum rather than an owned
tag-name string, so the vast majority of a listing response's XML (`Size`,
`LastModified`, `ETag`, `StorageClass`, ...) is skipped without allocating.

The `async-tokio` feature is **not** enabled — `parse_list_xml` operates synchronously
over the already-fully-buffered response body (`Bytes`, from `http_ingest::Client::list_prefix`),
so there is no async XML stream to support.

## Consequences
- `quick-xml` is pure Rust with no transitive C dependencies, permanently `0.x` but
  widely deployed and stable. The dependency audit behind ADR-0014 rates it low risk.
- Dropping `async-tokio` removes that feature's transitive dependencies from the graph;
  the response body is already fully materialized by the time XML parsing starts, so
  nothing is lost.
- `s3_poll.rs` remains the only place in the workspace that depends on `quick-xml`.
  `http_ingest` deliberately does not parse XML — see ADR-0014's decision to have
  `list_prefix` return raw `Bytes` rather than a `radar-workstation`-domain
  `ListResponse`, specifically to avoid dragging this dependency into the transport crate.

## Erratum (added during dependency-inventory remediation, 2026-07-31)

9. **Upgraded to `quick-xml 0.41`, was `0.37.5` at this ADR's original writing.**
   `cargo deny check` was wired up for the first time in this session and immediately
   flagged RUSTSEC-2026-0194 (a quadratic-time duplicate-attribute check in
   `BytesStart::attributes()`) and RUSTSEC-2026-0195 (unbounded allocation in
   `NsReader`'s namespace resolution), both against `quick-xml 0.37.5` — which parses
   `ListObjectsV2` responses, i.e. untrusted network input, exactly the threat model both
   advisories describe. Checked before acting: `parse_list_xml` never calls
   `.attributes()` and never uses `NsReader`, so neither advisory's affected path is
   reachable in this codebase as written. Upgraded to `0.41` (the fixed version) anyway
   rather than filing an exemption — dropping a known-vulnerable pin without a specific
   reason to keep it is the conservative move under Principle 4 (Security as
   First-Class). The upgrade was not purely mechanical: `0.41` changed how entity
   references are surfaced from the event stream, which required a corresponding fix in
   `parse_list_xml` — a `GeneralRef` accumulation path — guarded by two regression tests.
   Confirmed against live S3 both before and after. Full account:
   `docs/plans/dependency-inventory-remediation.md` §9.3.
