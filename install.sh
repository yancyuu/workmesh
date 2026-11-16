#!/bin/sh
set -eu

command -v cargo >/dev/null 2>&1 || {
  echo "cargo is required. Install Rust from https://rustup.rs first." >&2
  exit 1
}

cargo install --git https://github.com/yancyuu/workmesh.git --locked
echo "Installed $(workmesh --version). Try: workmesh init"
