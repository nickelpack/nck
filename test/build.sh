#!/usr/bin/env bash
set -euo pipefail

# [ -e rootfs ] && rm -rf rootfs

# container=$(buildah from docker.io/library/debian:12-slim)
# trap "buildah rm $container" EXIT

# buildah run $container apt update
# buildah run $container apt install -y build-essential linux-headers-amd64 python3 gawk bison wget
# buildah run $container apt clean
# buildah run $container rm -r /usr/share/man /usr/share/doc /usr/share/locale /root /home /boot /media /mnt

# image=$(buildah commit $container)
# buildah rm $container
# trap "" EXIT

# [ -e rootfs.tar ] && rm rootfs.tar

# container=$(podman create $image)
# trap "podman rm $container" EXIT
# podman export $container -o rootfs.tar

# mkdir rootfs
# tar xf rootfs.tar -C rootfs
# rm rootfs.tar

(
  cd rootfs
  cargo run --bin nck -- archive create -o ../rootfs.nck -- *
)

zstd -T0 -19 --rm rootfs.nck -o rootfs.nck.zst
