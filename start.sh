#!/bin/bash

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

cd -- "$SCRIPT_DIR" || exit 1

systemd-cat -t venev-kanshi -- kanshi -c cfg/kanshi.conf &
kanshi_pid=$!

sleep 4

export RUST_LOG=${RUST_LOG:-info}
exec systemd-cat -t venev-ng -- target/release/venev-ng
# cargo run --release
