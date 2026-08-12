# Encrypted environment workflow

This repository commits environment values only as SOPS dotenv ciphertext at:

```text
env/enc/dev.env.enc
env/enc/prod.env.enc
```

Decrypted files live only under ignored, owner-only `env/dec/`. A root `.env`
may exist only as a managed relative symlink to `env/dec/dev.env` or
`env/dec/prod.env`. Git does not track empty directories, so `nix develop
./.nix` and `just bootstrap` create `env/enc` and `env/dec`; do not add
`.gitkeep` files.

## Provider ownership

The public schema in `.env.example` names the supported FCM, APNs, Expo, Web
Push, SendGrid, Twilio, NATS, authentication, and observability variables. It
contains no credentials. Provider credentials and sender identities are
server-owned and must never arrive in `PushJob` or `ContactJob` payloads.

For local or explicitly reviewed operator profiles, place values in an ignored
`env/dec/<name>.env` and run `just encrypt <name>`. Canonical production values
should remain in the deployment secret store and reach Kubernetes through its
protected secret-delivery path. `env/enc/prod.env.enc` is an optional protected
operator/runtime profile, not a second ungoverned production source of truth.

Do not copy one full-access provider key into multiple service repositories.
Issue least-privilege keys per environment and service, rotate any credential
that entered chat or another untrusted transcript, and use opaque secret
references in deployment manifests.

## Commands

```sh
nix develop ./.nix
just bootstrap
just seed dev
$EDITOR env/dec/dev.env
just encrypt dev
git add env/enc/dev.env.enc
just verify
just run dev
just lock
```

Ordinary changes should use `just edit dev|prod`. `just diff` reports only
changed variable names. Tools that do not require a file should use `just run`,
`just test-env`, or `just exec-env` so plaintext never reaches disk.

## Release gate

The production rule requires at least two recipients, a recipient used only by
production, and a recipient set distinct from development. This proves policy
shape only. Before production reliance, verify independent custody and a
protected decryptability witness, then re-key ciphertext with `sops updatekeys`.

Never decrypt in `docker build`, untrusted pull-request CI, logs, test fixtures,
artifacts, crash reports, or Linear/GitHub comments. Provider canaries must use
protected environments and must not print request headers, recipients, bodies,
device endpoints, callback secrets, or raw upstream responses.
