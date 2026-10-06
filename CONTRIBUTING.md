# Contributing to agentic_ssh

Thank you for contributing to `agentic_ssh`! This guide covers our local development setup, verification commands, and standard protocol for reviewing and landing Pull Requests (including automated dependency PRs from Renovate).

---

## 🛠️ Prerequisites

- **Rust**: Latest stable toolchain (`rustup` or via Nix devShell).
- **Nix**: Flakes-enabled Nix (Determinate Systems or NixOS).
- **Just**: Command runner (`just`) for project shortcuts.
- **GitHub CLI**: `gh` for PR management.
- **Beads**: `br` for issue tracking.

To enter the project development shell with all tools in scope:
```bash
nix develop
```

---

## 🧪 Local Verification Gate

Before submitting or merging any code, all changes must satisfy the verification gate:

| Command | What it does |
|---|---|
| `just fmt` | Formats code with `rustfmt` |
| `just clippy` | Runs `cargo clippy --all-targets` |
| `just test` | Runs the test suite |
| `just check` | Runs `cargo fmt --check`, `clippy`, and `test` in sequence |
| `just check-nix` | Verifies `nix build --no-link` |
| `just release-check` | Runs the full verification pipeline (`check` + `check-nix`) |

---

## ❄️ Nix Builds & the `cargoHash` Lifecycle

`agentic_ssh` is packaged as a Nix flake using `pkgs.rustPlatform.buildRustPackage`.

In `flake.nix`, dependencies are vendored via a fixed-output derivation governed by `cargoHash`:
```nix
cargoHash = "sha256-...";
```

### The Rule
**Any time `Cargo.lock` changes (dependency addition, update, or removal), `cargoHash` in `flake.nix` MUST be updated.**

If `cargoHash` is not updated, `nix build` and GitHub Actions Nix CI jobs will fail with a vendor mismatch error.

### Auto-Updating `cargoHash`
We have an automated Justfile helper to handle this:
```bash
just update-nix-hash
```

This recipe:
1. Detects `Cargo.lock` drift.
2. Forces Nix to evaluate the new crate vendor archive.
3. Automatically writes the newly computed hash into `flake.nix`.
4. Confirms that `nix build --no-link` succeeds.

---

## 🤖 Reviewing & Testing Pull Requests (e.g. Renovate)

Automated bots like Renovate update `Cargo.toml` and `Cargo.lock`, but they do not run Nix. As a result, dependency PRs will pass standard Rust checks but fail the `Nix Build` CI check until `flake.nix` is updated.

Follow this protocol to review and merge:

### Step 1: Inspect the PR
Inspect the diff and release notes:
```bash
gh pr view <PR_NUMBER>
gh pr diff <PR_NUMBER>
```

### Step 2: Check Out the PR Locally
Check out the remote branch into your local workspace:
```bash
gh pr checkout <PR_NUMBER>
```

### Step 3: Verify the Rust Code
Ensure the dependency update introduces no breaking changes or new clippy warnings:
```bash
just check
```

### Step 4: Update the Nix Hash
Update `flake.nix` to match the new `Cargo.lock`:
```bash
just update-nix-hash
just check-nix
```

### Step 5: Push and Verify CI
Commit the updated `flake.nix` to the PR branch and push:
```bash
git add flake.nix Cargo.lock
git commit -m "chore(nix): update cargoHash"
git push
```

GitHub Actions will re-trigger and turn green.

### Step 6: Merge
Once CI passes:
```bash
gh pr merge <PR_NUMBER> --squash --delete-branch
```

---

## 📋 Beads Issue Protocol

Issues and tasks are tracked via Beads (`br` / `bd`):
```bash
br ready              # Find open, unblocked work
br update <id> --status=in_progress
br close <id> --reason="Completed"
br sync --flush-only  # Always flush DB to JSONL before ending session
```
