#!/bin/sh
# Builds a release binary and a .desktop launcher for Linux.
# Needs: libasound2-dev libudev-dev libxkbcommon-dev pkg-config (Debian/Ubuntu names).
# For the call camera: sudo apt install v4l2loopback-dkms
#   then: sudo modprobe v4l2loopback exclusive_caps=1 card_label="Studio Record"
set -e
cd "$(dirname "$0")/.."
cargo build --release
mkdir -p dist/linux
cp target/release/studio-record dist/linux/
cat > dist/linux/studio-record.desktop <<D
[Desktop Entry]
Type=Application
Name=Studio Record
Comment=Lecture studio: camera, screen and iPad scenes
Exec=studio-record
Categories=AudioVideo;Video;Education;
D
echo "Built dist/linux"
