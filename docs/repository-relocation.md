# Repository publication to Fanwaave

The canonical product repository is:

```text
https://github.com/fanwaave/push-notification-server.rs
```

The historical/source repository remains independently available at:

```text
https://github.com/ORESoftware/push-notification-server.rs
```

This project was published by copying the complete source branch and tag graph into a new Fanwaave repository. It was **not** transferred, forked, redirected, or configured as an automatic mirror.

## Stable publication evidence

- Source repository ID: `1314172992`
- Source `main` copied at publication: `f6c11c8ca8d5454ecfcf46b3df4625276d8a0e7d`
- Destination repository ID: `1324425321`
- Destination provenance PR: `fanwaave/push-notification-server.rs#1`
- Destination provenance merge: `725bb3d34e12b448541241cec73f1e0a89b7893c`

The repositories share history through the publication point and may diverge afterward. New product work, releases, packages, issues, and deployment references belong to Fanwaave.

## Existing clones

HTTPS:

```bash
git remote set-url origin https://github.com/fanwaave/push-notification-server.rs.git
```

SSH:

```bash
git remote set-url origin git@github.com:fanwaave/push-notification-server.rs.git
```

Verify after changing the remote:

```bash
git fetch --prune origin
git remote -v
git rev-parse origin/main
```

Do not assume source and destination `main` remain equal after publication. Compare explicit SHAs when auditing migration or backport work.

## Operational follow-through

Publication is complete, but production cutover remains tracked in destination issues `#2`, `#3`, and `#4` plus Linear issues `DEN-1874` and `DEN-1875`.

The cutover must:

- publish and deploy the digest-addressable Fanwaave GHCR image;
- update Kubernetes, submodules, image coordinates, badges, service catalogs, and runbooks;
- verify GitHub App, Actions, package, environment, secret, webhook, and release permissions in Fanwaave;
- record exact source, image, GitOps, and live pod identities;
- preserve the ORESoftware repository as an independent historical/source copy unless a separate reviewed retirement decision is approved.
