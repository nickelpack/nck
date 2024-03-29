#!/usr/bin/env bash
set -euo pipefail

[ -e rootfs ] && rm -rf rootfs

container=$(buildah from docker.io/library/debian:12-slim)
trap "buildah rm $container" EXIT

buildah run $container apt update
buildah run $container apt install -y build-essential linux-headers-amd64 python3 gawk bison wget
buildah run $container apt clean
buildah run $container rm -r /usr/share/man /usr/share/doc /usr/share/locale /root /home /boot /media /mnt

image=$(buildah commit $container)
buildah rm $container
trap "" EXIT

[ -e rootfs.tar ] && rm rootfs.tar

container=$(podman create $image)
trap "podman rm $container" EXIT
podman export $container -o rootfs.tar
# zstd -T0 -19 --rm rootfs.tar -o rootfs.tar.zst
gzip --best rootfs.tar
