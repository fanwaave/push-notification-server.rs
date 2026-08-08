# Immutable container publication and GitOps rollout

The canonical runtime image is:

```text
ghcr.io/fanwaave/push-notification-server
```

Production deployments must use an exact digest reference:

```text
ghcr.io/fanwaave/push-notification-server@sha256:<64 lowercase hex characters>
```

Mutable tags such as `main`, `latest`, and `sha-*` are discovery conveniences only. They are not deployment identities.

## Publication contract

`.github/workflows/container-image.yml` runs for container-affecting pull requests and for pushes to `main` or version tags.

For pull requests it:

1. Builds a local Linux/amd64 candidate from digest-pinned Rust and Debian bases.
2. Verifies the configured non-root user and entrypoint.
3. Runs the image with a read-only root filesystem and verifies `/healthz`.
4. Fails on fixed HIGH or CRITICAL OS or library vulnerabilities.

For trusted pushes it additionally:

1. Publishes `ghcr.io/fanwaave/push-notification-server` with OCI source and revision labels.
2. Attaches a software bill of materials and maximum BuildKit provenance.
3. Resolves the manifest digest returned by Buildx.
4. Pulls that exact `image@digest` back from GHCR.
5. Re-verifies the runtime user, entrypoint, and source revision label.
6. Scans the exact published digest.
7. Uploads a 90-day JSON evidence artifact named `push-notification-image-digest-<run>-<attempt>`.

The evidence document includes the source SHA, workflow run and attempt, image, digest, exact image reference, and the runtime/health/scan assertions required before GitOps may consume it.

## Kubernetes cutover

The existing `ORESoftware/k8s-cluster` deployment builds Rust inside the pod from a host path or cloned source. Replace that design with the exact image reference from trusted publication evidence.

The cutover pull request must:

- remove the Rust builder runtime, source checkout, hostPath mount, cargo caches, `GH_PAT`, and startup compilation;
- set the container image to the reviewed Fanwaave `image@sha256:...` reference;
- retain the `dd-push-notification-server-config` ConfigMap and `dd-push-notification-server-secrets` ExternalSecret wiring;
- run as UID/GID `10001`, disallow privilege escalation, drop all capabilities, and use a read-only root filesystem with only bounded temporary storage when required;
- replace the build-sized startup probe with normal health and readiness timing;
- update `.gitmodules`, submodule documentation, service catalogs, and deployment provenance to the Fanwaave repository;
- prove the Argo CD application is Synced and Healthy at the exact GitOps revision;
- record the live pod `imageID`, `/healthz`, `/readyz`, authenticated ingestion denial/acceptance, rollback baseline, and rollback or roll-forward evidence.

Tracking:

- GitHub: `fanwaave/push-notification-server.rs#4`
- Linear: `DEN-1874`

## Rollback

Rollback changes only the GitOps digest to the last known-good reviewed digest. Never rebuild inside the cluster and never recover by changing a mutable tag. Record both old and new digests, the GitOps commit, Argo CD sync/health, pod image IDs, and probe evidence.
