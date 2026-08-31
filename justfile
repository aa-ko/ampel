# ampel — drive a Cleware USB traffic light from Rust.
#
# `just` on its own prints this summary and every recipe.

arm_target := "aarch64-unknown-linux-gnu"

# cross mounts ~/.cargo, so host-only settings there (sccache, clang/mold) get
# looked up inside a container that has none of them; see Cross.toml.
cross_env := "RUSTC_WRAPPER= CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=cc CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="

alias b := build
alias d := deploy
alias t := test

# List the available recipes
default:
    @just --list --unsorted

# Build both crates (release); the vendor C++ is compiled by cleware/build.rs
build:
    cargo build --release

# Run all workspace tests
test:
    cargo test

# Cross-compile co2ampel for a 64-bit Raspberry Pi (needs `cargo install cross` + docker)
arm:
    {{cross_env}} cross build --release --target {{arm_target}} -p co2ampel
    @echo "-> target/{{arm_target}}/release/co2ampel"

# Build for ARM and copy everything the Pi needs, e.g. `just deploy pi:~/co2ampel`
deploy target: arm
    #!/usr/bin/env sh
    set -eu
    case "{{target}}" in
        *:*) ;;
        *) echo "usage: just deploy <ssh-host>:<path>" >&2; exit 2 ;;
    esac
    host=$(echo "{{target}}" | cut -d: -f1)
    path=$(echo "{{target}}" | cut -d: -f2-)
    # Unquoted on purpose: a leading ~ has to be expanded by the remote shell.
    if ! ssh "$host" "mkdir -p $path"; then
        echo "could not create $path on $host — does it already exist as a file?" >&2
        exit 1
    fi
    scp target/{{arm_target}}/release/co2ampel \
        packaging/99-cleware.rules \
        packaging/co2ampel.service \
        packaging/co2ampel.env \
        packaging/setup.sh \
        "$host:$path/"
    echo
    echo "Copied. On the Pi:"
    echo "  cd $path && sudo ./setup.sh <user>"

# Run the CO2 light, e.g. `just co2 --host pi --topic buero-nord/co2sensor/co2`
co2 *args: build
    ./target/release/co2ampel {{args}}

# Turn the light off
off: build
    ./target/release/co2ampel --off

# Install the udev rule for non-root access to the light (needs sudo)
install-udev:
    sudo install -m 0644 packaging/99-cleware.rules /etc/udev/rules.d/99-cleware.rules
    sudo udevadm control --reload-rules
    sudo udevadm trigger
    @echo "Installed. Re-plug the light for the rule to apply."

# Remove build artifacts
clean:
    cargo clean
