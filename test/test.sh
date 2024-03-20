#!/usr/bin/env bash
set -euo pipefail

pwd=$(pwd)
host=http://socket

fetch() {
  echo curl "$@" >&2
  curl "$@"
}

req() {
  fetch -si --unix-socket /var/nck/daemon.sock "$@" | sed 's#\r##g'
}

upload_file() {
  local file=$1
  local uploaded_file=$(req -X POST "${host}${formula_url}/file" --data-binary "@-" < "$file")
  echo "$uploaded_file" >&2
  awk -v FS=': ' '/^etag/{print $2}' <<< "$uploaded_file" | sed -e 's#^"##' -e 's#"$##'
}

fetch_file() {
  local src=$1
  local int=$2
  if [ ! -e "/var/nck/store/files/$int" ]; then
    fetch "$src" -o /tmp/src
    local uploaded_file=$(req -X POST "${host}${formula_url}/file" --data-binary "@/tmp/src" -H "If-None-Match: \"${int}\"")
    awk -v FS=': ' '/^etag/{print $2}' <<< "$uploaded_file" | sed -e 's#^"##' -e 's#"$##'
  else
    echo "$int"
  fi
}

encode() {
  jq -rn --arg x "$0" '$x|@uri'
}

create_formula_response=$(req -X POST "$host/api/1/build")
formula_url=$(awk -v FS=": " '/^location/{print $2}' <<< "$create_formula_response")
echo "-- $formula_url --" 1>&2

echo "uploading rootfs"
integrity=$(upload_file "rootfs.tar.gz")

echo "uploading tar"
tar_url="https://busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox_TAR"
tar_int="blake3-43bvwtdwfeaxy6hfq4mohwlwbl73pr52tntlidmdlpnl2uzmsqhq"

fetch_file "$tar_url" "$tar_int"

echo "uploading gzip"
gunzip_url="https://www.busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox_GUNZIP"
gunzip_int="blake3-7h32xiopodjandasbz7vefv4pfl7ble377xr7nfcoqiuzdcj4upq"

fetch_file "$gunzip_url" "$gunzip_int"

request="{
  \"name\": \"bootstrap-0.0.1\",
  \"outputs\": [ \"out\" ],
  \"files\": [
    \"${integrity}\",
    \"${tar_int}\",
    \"${gunzip_int}\"
  ],
  \"actions\": [
    {
      \"action\": \"set\",
      \"name\": \"TMP\",
      \"value\": \"/tmp\"
    },
    {
      \"action\": \"set\",
      \"name\": \"TMPDIR\",
      \"value\": \"/tmp\"
    },
    {
      \"action\": \"set\",
      \"name\": \"TEMP\",
      \"value\": \"/tmp\"
    },
    {
      \"action\": \"set\",
      \"name\": \"TEMPDIR\",
      \"value\": \"/tmp\"
    },
    {
      \"action\": \"set\",
      \"name\": \"HOME\",
      \"value\": \"/no-home\"
    },
    {
      \"action\": \"set\",
      \"name\": \"TERM\",
      \"value\": \"xterm-256color\"
    },
    {
      \"action\": \"set\",
      \"name\": \"PATH\",
      \"value\": \"/bin:/usr/bin:/sbin:/usr/sbin:/tmp/busybox\"
    },
    {
      \"action\": \"work_dir\",
      \"path\": \"/\"
    },
    {
      \"action\": \"link\",
      \"from\": \"/var/nck/store/files/${tar_int}\",
      \"to\": \"/tmp/busybox/tar\",
      \"executable\": true
    },
    {
      \"action\": \"link\",
      \"from\": \"/var/nck/store/files/${gunzip_int}\",
      \"to\": \"/tmp/busybox/gunzip\",
      \"executable\": true
    },
    {
      \"action\": \"exec\",
      \"path\": \"/tmp/busybox/tar\",
      \"args\": [ \"-xzf\", \"/var/nck/store/files/${integrity}\" ]
    }
  ]
}"

req -X POST "${host}${formula_url}/run" --data "$request" -H "Content-Type: application/json"
