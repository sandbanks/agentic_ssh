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
    if echo "$OUTPUT" | grep -q "got:[[:space:]]*sha256-"; then
        NEW_HASH=$(echo "$OUTPUT" | grep -o 'got:[[:space:]]*sha256-[^[:space:]]*' | awk '{print $2}')
        echo "🔄 Updating flake.nix with new hash: $NEW_HASH"
        sed -i.bak -E "s|cargoHash = \"sha256-[^\"]+\";|cargoHash = \"$NEW_HASH\";|" flake.nix
        rm -f flake.nix.bak
        echo "✅ flake.nix updated successfully!"
    else
        echo "✅ Nix build is already up to date!"
    fi

# Full release verification (checks + Nix build)
release-check: check check-nix
