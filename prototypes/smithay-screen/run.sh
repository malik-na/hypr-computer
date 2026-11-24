#!/bin/sh
# Run one measurement inside the desktop session:  ./run.sh gles|pixman [--pixman-dmabuf]
export XDG_RUNTIME_DIR=/run/user/$(id -u)
export WAYLAND_DISPLAY=${HOST_WAYLAND_DISPLAY:-wayland-1}
export HYPRLAND_INSTANCE_SIGNATURE=$(ls -t "$XDG_RUNTIME_DIR/hypr" | head -1)
here=$(cd "$(dirname "$0")" && pwd)
tag=$1$(echo "$2" | tr -d ' ')
exec python3 "$here/measure.py" --renderer "$1" $2 --work "${HC_WORK:-$here/../run}/$tag"
