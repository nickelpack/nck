#!/usr/bin/env bash
set -euo pipefail

[ -e rootfs ] && rm -rf rootfs

container=$(buildah from docker.io/library/debian:12-slim)
buildah run $container apt update
buildah run $container apt install -y build-essential linux-headers-amd64 python3 gawk bison wget
trap "buildah rm $container" EXIT

buildah run $container apt clean
buildah run $container rm -r /usr/share/man /usr/share/doc /usr/share/locale /root /home /boot /media /mnt
image=$(buildah commit $container)

buildah rm $container
trap "" EXIT

[ -e npk-build.tar ] && rm npk-build.tar

container=$(podman create $image)
trap "podman rm $container" EXIT
podman export $container -o npk-build.tar

mkdir -p rootfs
tar xf npk-build.tar -C rootfs
