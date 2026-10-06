# agentic_ssh Justfile 🦀🛰️

default:
    @just --list

# Format code with rustfmt
fmt:
    cargo fmt

# Run Clippy lints
clippy:
    cargo clippy --all-targets

# Run the test suite
test:
    cargo test

# Fast local check (fmt check + clippy + test)
check:
    cargo fmt --check
    cargo clippy --all-targets
    cargo test

# Verify local Nix build & cargoHash
check-nix:
    nix build --no-link

# Auto-update flake.nix cargoHash if mismatched
update-nix-hash:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "🔍 Checking Nix cargoHash..."
    OUTPUT=$(nix build --no-link 2>&1 || true)
    if echo "$OUTPUT" | grep -q "ERROR: cargoHash or cargoSha256 is out of date"; then
        echo "⚠️  Cargo.lock changed; resetting cargoHash to recompute vendor hash..."
        sed -i.bak -E 's|cargoHash = "sha256-[^"]*";|cargoHash = "";|' flake.nix
        rm -f flake.nix.bak
        OUTPUT=$(nix build --no-link 2>&1 || true)
    fi
    NEW_HASH=$(echo "$OUTPUT" | grep -E 'got:[[:space:]]+sha256-[A-Za-z0-9+/=]{44}' | head -n 1 | sed -E 's/.*(sha256-[A-Za-z0-9+/=]{44}).*/\1/' || true)
    if [ -n "$NEW_HASH" ]; then
        echo "🔄 Updating flake.nix with new hash: $NEW_HASH"
        sed -i.bak -E "s|cargoHash = \"[^\"]*\";|cargoHash = \"$NEW_HASH\";|" flake.nix
        rm -f flake.nix.bak
        echo "✅ flake.nix updated successfully!"
    else
        if nix build --no-link >/dev/null 2>&1; then
            echo "✅ Nix build is already up to date!"
        else
            echo "❌ Failed to determine new cargoHash. nix build output:"
            echo "$OUTPUT"
            exit 1
        fi
    fi

# Full release verification (checks + Nix build)
release-check: check check-nix
