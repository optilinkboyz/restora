# Code signing — reference notes

**Scope note, upfront:** nothing in this file has been executed or
verified — it can't be, without a real code-signing certificate for each
platform, which this project doesn't have. What follows is accurate,
current (as of writing) guidance on what each platform actually requires,
written so you have a correct starting point rather than having to
research this from scratch. Treat it as a checklist, not a
tested-and-working script.

## Why this matters at all

An unsigned build will trigger real friction for anyone who downloads it:
Windows SmartScreen shows a "Windows protected your PC" warning requiring
an extra click through; macOS Gatekeeper refuses to open it at all
without an explicit right-click → Open override (and won't even offer
that override for a downloaded, unnotarized app past a certain macOS
version); most antivirus heuristics are more suspicious of unsigned
binaries generally, and *especially* suspicious of one that requests raw
disk access — exactly what this app does — a pattern shared with
ransomware and disk-wiping malware.

## Windows

Requires a code-signing certificate — either an OV (Organization
Validation) or EV (Extended Validation) certificate from a recognized CA
(DigiCert, Sectigo, etc.). EV certificates give immediate SmartScreen
trust; OV certificates need to build up reputation over time/downloads
before SmartScreen stops warning.

Tauri's bundler signs automatically if configured. In
`desktop/src-tauri/tauri.conf.json`, under `bundle.windows`:

```json
"windows": {
  "certificateThumbprint": "YOUR_CERT_THUMBPRINT",
  "digestAlgorithm": "sha256",
  "timestampUrl": "http://timestamp.digicert.com"
}
```

The certificate itself needs to already be installed in the Windows
certificate store (or referenced via a `.pfx` + password through
`signtool` directly) on whatever machine runs the build — this isn't
something `cargo tauri build` can supply on its own.

## macOS

Two separate steps, both required for a smooth install experience:

1. **Code signing** — requires an active Apple Developer Program
   membership (paid, ~$99/year) and a "Developer ID Application"
   certificate. Configure in `tauri.conf.json` under `bundle.macOS`:
   ```json
   "macOS": {
     "signingIdentity": "Developer ID Application: Your Name (TEAMID)"
   }
   ```
2. **Notarization** — a separate Apple service that scans the signed
   binary for malware and issues a ticket Gatekeeper checks at launch.
   Without this, even a signed app shows a Gatekeeper warning on modern
   macOS. Tauri can drive this automatically given Apple ID credentials
   supplied as environment variables (`APPLE_ID`, `APPLE_PASSWORD` —
   an app-specific password, not your real Apple ID password —
   and `APPLE_TEAM_ID`) at build time.

## Linux

No OS-level code-signing gate equivalent to Windows/macOS exists for a
general binary. What Linux distribution channels actually expect instead:

- **AppImage**: no signing required to run, though GPG-signing the
  AppImage itself (a detached `.sig` file) is common practice so users
  can verify provenance.
- **Flatpak/Snap**: signing is handled by the respective store
  infrastructure (Flathub, the Snap Store) at submission time, not by
  you directly.
- Tauri's Debian (`.deb`) and RPM bundle targets don't require signing to
  install locally, though a proper apt/yum repository would want the
  repo metadata GPG-signed — a repository-hosting concern, not a
  per-build one.

## CI integration (once you have real certificates)

The `desktop-build` job in `.github/workflows/ci.yml` currently only
builds the desktop shell, unsigned — it exists to catch compile breakage,
not to produce signed release artifacts. Adding real signing to CI means
storing the certificate/credentials as GitHub Actions secrets (`Settings
→ Secrets and variables → Actions`) and referencing them via environment
variables in a release-specific workflow, kept separate from the
every-push `ci.yml` so routine commits don't require signing credentials
to be present at all.
