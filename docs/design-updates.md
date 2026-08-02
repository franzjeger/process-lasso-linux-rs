# In-app updates

Argus-Lasso checks GitHub for a newer release and can replace its own binary,
so a user on the per-user install does not have to re-run the install script
after every release.

## How it works

1. **Check** — `GET /repos/franzjeger/process-lasso-linux-rs/releases/latest`.
   The tag is compared to `CARGO_PKG_VERSION` by dotted numeric components;
   a suffix like `-rc1` parses as `0`, so a pre-release never outranks the
   release it precedes.
2. **Pick the asset** — the release publishes
   `argus-lasso-<version>-<arch>-linux.tar.gz` plus a `.sha256`. The arch
   comes from `std::env::consts::ARCH`, so an aarch64 build never downloads
   the x86_64 tarball.
3. **Verify** — the tarball is hashed and compared to the published checksum.
   A mismatch aborts before anything is written.
4. **Install** — the binary is extracted, written next to the running one as
   `.argus-lasso.update`, marked executable, and `rename(2)`d over the
   target. Rename is atomic and works on a running binary; writing over it
   in place fails with `ETXTBSY`.
5. **Restart** — `exec(2)` replaces the process image, keeping the same PID.
   There is no window where neither the old nor the new process is running.

All network and disk work happens on a worker thread; the UI polls a channel
once per frame.

## Where it deliberately does nothing

**System-wide installs.** Before downloading, the updater probes whether the
directory holding the binary is writable by creating and removing a temp
file. A distro package or a `/usr/local` install is root-owned, so the probe
fails and the user is told to use their package manager instead. The probe
is a write test rather than a mode-bit check because mode bits miss ACLs and
read-only mounts.

## Known gap: the checksum is not a signature

The tarball and its `.sha256` come from the same release. That combination
detects a truncated or corrupted download — the failure mode that actually
happens — but it does not detect a tampered release, because anyone who
could replace the tarball could replace the checksum beside it.

Closing that properly needs a detached signature over the tarball, made with
a key that does not live in the repository or in the release, and verified
against a public key compiled into the binary. Two workable routes:

- **minisign / signify** — sign in the release workflow with a key held in
  repository secrets, ship the `.minisig` as a release asset, and verify with
  a bundled public key. Simple and self-contained; the private key's exposure
  is whatever the CI secret store gives it.
- **Sigstore / cosign keyless** — sign with an ephemeral key bound to the
  workflow identity and verify against the transparency log. No long-lived
  private key, at the cost of a heavier verification dependency.

Until one of those is in place, the honest description of the current
guarantee is "this came over TLS from the GitHub release and arrived intact",
not "this is authentic".

## Trust roots

`ureq` is built with `rustls` and the bundled Mozilla root set, so
verification does not depend on the host's certificate store being sane. The
practical consequence is that a TLS-intercepting proxy will fail the check
with an `UnknownIssuer` error rather than silently trusting the interceptor —
which is the behaviour we want for a self-updater.
