#!/bin/bash

# wlr-randr
# wlr-randr --output HDMI-A-1 --mode 1920x1080 --pos 0,1080
# wlr-randr --output DP-1 --mode 1920x1080 --pos 0,0

cd "$HOME/venevit"

systemd-cat -t venev-kanshi -- kanshi -c cfg/kanshi.conf &
kanshi_pid=$!

sleep 4

export RUST_LOG=${RUST_LOG:-info}
exec systemd-cat -t venev-ng -- target/release/venev-ng
# cargo run --release
