# Historical source preservation

`materialize-code-first-openapi-clean.yml.txt` preserves the exact workflow
restored to main at `582e9ea3aa06480878d7141a8791b989741ff141`. It originated
in `d8cfa06a2d1b31a3fb2edcfbd054d7181bf4c5e7`. The bulk merge restored it even though
the executable OpenAPI product routes and their normal CI are already present.

This was a temporary source-publication mechanism for an earlier branch. It
imports an old snapshot and rewrites branch history. The destination's
`AGENTS.md` excludes source-publication workflows; preserve this historical
source as text, outside GitHub Actions, rather than executing it again.

Use the maintained Rust routes, OpenAPI fixtures, HTTP tests, and browser tests
for further changes. Review any unique historical test or documentation before
porting it semantically. This archive neither certifies the old snapshot nor
authorizes its cleanup or force-push operations. The original Git history and
branches remain intact.
