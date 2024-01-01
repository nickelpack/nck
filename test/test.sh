#!/usr/bin/env bash
set -euo pipefail

pwd=$(pwd)
host=http://socket

req() {
  curl -si --unix-socket /var/nck/daemon.sock "$@"
}

upload_file() {
  local file=$1
  local uploaded_file=$(req -X POST "${host}${formula_url}/upload" --data-binary "@-" < "$file" | sed 's#\r##g')
  awk -v FS=': ' '/^etag/{print $2}' <<< "$uploaded_file" | sed -e 's#^"##' -e 's#"$##'
}

encode() {
  jq -rn --arg x "$0" '$x|@uri'
}

extract_file() {
  local source=$(upload_file "$1")
  local dest=$2
  local de=$3
  local encoded=$(encode "$dest")
  local result=$(
    req -X POST "${host}${formula_url}/action/extract" \
      --form "source=${source}" \
      --form "dest=${dest}" \
      --form "compression=${de}" \
      | sed 's#\r##g'
  )
  echo "$source: $result"
}

req -v -X POST "$host/api/1/spec"

create_formula_response=$(req -X POST "$host/api/1/spec" | sed 's#\r##g')
echo "$create_formula_response"
formula_url=$(awk -v FS=": " '/^location/{print $2}' <<< "$create_formula_response")
echo "-- $formula_url --" 1>&2

extract_file "rootfs.nck.zst" "/" "zstd"

echo "#!/bin/bash" > script
echo "/support/tar -xvf /rootfs.tar" >> script

req -X POST "${host}${formula_url}/action/execute" \
  --form "bin=/support/run" \
  --form "env[TMP]=/tmp" \
  --form "env[TMPDIR]=/tmp" \
  --form "env[TEMP]=/tmp" \
  --form "env[TEMPDIR]=/tmp" \
  --form "env[HOME]=/no-home" \
  --form "env[TERM]=xterm-256color" \
  --form "env[PATH]=/bin:/usr/bin:/sbin:/usr/sbin"

req -X POST "${host}${formula_url}/run"
