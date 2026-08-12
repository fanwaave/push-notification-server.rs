# Nix development shell

From the repository root:

```sh
nix develop ./.nix
just bootstrap
just verify
```

The flake pins Rust development tools together with the canonical
`ORESoftware/ores-sops` package, SOPS, age, and Just. Its shell hook creates the
ignored `env/enc` and owner-only `env/dec` directories and installs guarded Git
hooks. It does not select or decrypt an environment automatically.

Use `just run dev` to inject decrypted values directly into the server process.
Do not decrypt during `docker build`, untrusted pull-request CI, or artifact
packaging.
