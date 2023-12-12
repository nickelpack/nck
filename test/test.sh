#!/usr/bin/env bash
set -euo pipefail

pwd=$(pwd)
host=http://socket

req() {
  curl -si --unix-socket /var/nck/nck-daemon.socket "$@"
}

upload_file() {
  local file=$1
  local uploaded_file=$(req -X POST "${host}${formula_url}/write" --data-binary "@-" < "$file" | sed 's#\r##g')
  awk -v FS=': ' '/^etag/{print $2}' <<< "$uploaded_file" | sed -e 's#^"##' -e 's#"$##'
}

encode() {
  jq -rn --arg x "$0" '$x|@uri'
}

copy_file() {
  local source=$(upload_file "$1")
  local dest=$2
  local mode=$3
  local encoded=$(encode "$dest")
  local result=$(req -X POST "${host}${formula_url}/copy/${source}?to=$dest&executable=$mode" | sed 's#\r##g')
}

set_env() {
  req -X POST "${host}${formula_url}/env/$1" --data-binary "$2" -H "content-type: text/plain" | sed 's#\r##g'
}

build() {
  req -X POST "${host}${formula_url}/build/$1" | sed 's#\r##g'
}

create_formula_response=$(req -X POST $host/api/1/formulas/glibc-2.38 | sed 's#\r##g')
formula_url=$(awk -v FS=": " '/^location/{print $2}' <<< "$create_formula_response")
echo "-- $formula_url --"

for file in "support/"*; do
  echo "$file"
  copy_file "$file" "/$file" true
done

for file in "src/"* "rootfs.tar"; do
  echo "$file"
  copy_file "$file" "/$file" false
done

echo "#!/support/ash" > script
echo "/support/tar -xvf /rootfs.tar" >> script

copy_file "$pwd/script" "/support/run" true

set_env "TMP" "/tmp"
set_env "TMPDIR" "/tmp"
set_env "TEMP" "/tmp"
set_env "TEMPDIR" "/tmp"
set_env "HOME" "/no-home"
set_env "TERM" "xterm-256color"
set_env "PATH" "/bin:/usr/bin:/sbin:/usr/sbin"

build "support/run"
