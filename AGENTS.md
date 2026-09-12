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
- Build values, don't mutate them: functions return new values instead of filling `&mut` parameters or caller-owned collections. Deliberate exceptions on hot paths (token caches under a lock, pinned futures in `select!`) carry a `HOT-PATH (imperative by design)` comment with the reason. See [`docs/FUNCTIONAL-STYLE.md`](./docs/FUNCTIONAL-STYLE.md).

## Runtime and release boundary

- The canonical image is `ghcr.io/fanwaave/push-notification-server`.
- Production and GitOps manifests must use an exact `image@sha256:...` reference from the machine-readable digest evidence artifact produced by `.github/workflows/container-image.yml`.
- Do not deploy mutable tags such as `main`, `latest`, or `sha-*`.
- Preserve the non-root runtime, read-only-root-filesystem compatibility, health probe, SBOM, provenance, exact-digest pull verification, and HIGH/CRITICAL vulnerability gate.
- Source-publication workflows and temporary credential handoff scripts do not belong in this destination repository.

## Nested instructions

Before editing, walk upward from `$PWD` to the filesystem root and apply every relevant `AGENTS.md`, from broadest to most specific.

## Repository-local Git worktrees

- Create or use a Git worktree only when the human operator explicitly authorizes it for the current task. Concurrency or a dirty checkout is not permission by itself.
- Put every authorized worktree at `<repository-root>/tmp/worktrees/<name>`; from the repository root, use `./tmp/worktrees/<name>`. Never place worktrees beside repositories or organization directories.
- Keep `tmp`, `temp`, `tmp/worktrees`, and `temp/worktrees` ignored in the repository-root `.gitignore`. Do not commit files from those directories.
- Relocate or remove a worktree only when the operator explicitly requests it. Before removal, preserve and publish intended changes, verify its commit is represented on the target branch, and confirm there are no tracked, untracked, ignored-sensitive, or in-use files that must survive. Remove it with `git worktree remove <path>` without `--force`; never delete a worktree directory with `rm`.
