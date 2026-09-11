# Agent instructions

## Repository and tracking

- Canonical repository: `github.com/fanwaave/push-notification-server.rs`
- Historical source copy: `github.com/ORESoftware/push-notification-server.rs`
- Linear project: `github.com/fanwaave`
- Repository publication and cutover: `DEN-1874`
- Reliability, receipts, retry, observability, and recovery: `DEN-1875`
- Destination cutover issues: `fanwaave/push-notification-server.rs#2`, `#3`, and `#4`

The ORESoftware repository remains an independent historical/source copy. Do not describe it as a redirect, transfer source, or automatic mirror. New product work, releases, packages, and deployment references belong to Fanwaave.

## Git workflow

- Work from focused feature branches cut from current `main` and use pull requests.
- Avoid git rebase in favor of git merge.
- Sync with remote before and after material work.
- Resolve git conflicts semantically: do not merely pick one side. Preserve compatible behavior, contracts, tests, documentation, and security boundaries from both sides.
- After resolving conflicts, scan the complete worktree for conflict markers (`<<<<<<<`, `=======`, `>>>>>>>`) and rerun every affected contract.
- Never force-push shared branches, rewrite reviewed history, or bypass exact-head checks.
- Never commit secrets, production device tokens, Web Push capability URLs, provider private keys, recipient addresses, or phone numbers.

## Runtime and release boundary

- The canonical image is `ghcr.io/fanwaave/push-notification-server`.
- Production and GitOps manifests must use an exact `image@sha256:...` reference from the machine-readable digest evidence artifact produced by `.github/workflows/container-image.yml`.
- Do not deploy mutable tags such as `main`, `latest`, or `sha-*`.
- Preserve the non-root runtime, read-only-root-filesystem compatibility, health probe, SBOM, provenance, exact-digest pull verification, and HIGH/CRITICAL vulnerability gate.
- Source-publication workflows and temporary credential handoff scripts do not belong in this destination repository.

## Nested instructions

Before editing, walk upward from `$PWD` to the filesystem root and apply every relevant `AGENTS.md`, from broadest to most specific.
