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

mkdir -p support
curl https://busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox_ASH -o support/ash
curl https://busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox_TAR -o support/tar

mkdir -p src
curl https://ftp.gnu.org/gnu/glibc/glibc-2.38.tar.xz -o src/glibc-2.38.tar.xz
curl http://mirror.rit.edu/gnu/gcc/gcc-13.2.0/gcc-13.2.0.tar.xz -o src/gcc-132.2.0.tar.xz
