#!/bin/sh
set -eu

root="$(mktemp -d)"
trap 'rm -rf "$root"' 0

version="$(awk -F '"' '/^version = / { print $2; exit }' Cargo.toml)"
case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-musl" ;;
  *) exit 0 ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch="aarch64" ;;
  x86_64 | amd64) arch="x86_64" ;;
  *) exit 0 ;;
esac
archive_name="ask-${arch}-${os}.tar.gz"

install_dir="$root/install dir"
mkdir -p "$root/package" "$root/release" "$root/bin" "$install_dir"

for invalid_version in 01.2.3 1.2.3-beta.1 1..2 v+1.0.0; do
  if ASK_INSTALL_DIR="$install_dir" sh ./install.sh "$invalid_version" \
    > "$root/invalid-stdout" 2> "$root/invalid-stderr"; then
    exit 1
  fi
  grep -F "ask: invalid version: $invalid_version" "$root/invalid-stderr" >/dev/null
done

cat > "$root/package/ask" <<EOF
#!/bin/sh
printf '%s\n' '$version'
EOF
chmod 755 "$root/package/ask"
cp README.md LICENSE "$root/package/"
tar -C "$root/package" -czf "$root/release/$archive_name" ask README.md LICENSE

cat > "$install_dir/ask" <<'EOF'
#!/bin/sh
printf '0.0.0\n'
EOF
chmod 755 "$install_dir/ask"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$root/release" && sha256sum "$archive_name" > "$archive_name.sha256")
else
  (cd "$root/release" && shasum -a 256 "$archive_name" > "$archive_name.sha256")
fi

cat > "$root/bin/curl" <<'EOF'
#!/bin/sh
set -eu
destination=""
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      shift
      destination="$1"
      ;;
    https://*) url="$1" ;;
  esac
  shift
done
[ -n "$destination" ]
[ -n "$url" ]
case "$url" in
  "https://github.com/benja/ask/releases/download/v$ASK_TEST_VERSION/"*) ;;
  *) exit 1 ;;
esac
cp "$ASK_TEST_RELEASE/${url##*/}" "$destination"
EOF
chmod 755 "$root/bin/curl"

  PATH="$root/bin:$PATH" \
  ASK_TEST_RELEASE="$root/release" \
  ASK_TEST_VERSION="$version" \
  ASK_INSTALL_DIR="$install_dir" \
  sh ./install.sh "$version" \
  > "$root/stdout" 2> "$root/stderr"

test ! -s "$root/stdout"
test -x "$install_dir/ask"
test "$("$install_dir/ask" --version)" = "$version"
grep -F "installed ask $version to $install_dir/ask" "$root/stderr" >/dev/null

mkdir "$root/corrupt-release"
cp "$root/release/$archive_name"* "$root/corrupt-release/"
printf 'corrupt' >> "$root/corrupt-release/$archive_name"
mkdir "$root/corrupt-install"
cp "$root/package/ask" "$root/corrupt-install/ask"
if PATH="$root/bin:$PATH" \
  ASK_TEST_RELEASE="$root/corrupt-release" \
  ASK_TEST_VERSION="$version" \
  ASK_INSTALL_DIR="$root/corrupt-install" \
  sh ./install.sh "$version" \
  > "$root/corrupt-stdout" 2> "$root/corrupt-stderr"; then
  exit 1
fi
test "$("$root/corrupt-install/ask" --version)" = "$version"
grep -F "ask: checksum verification failed" "$root/corrupt-stderr" >/dev/null

mkdir "$root/mismatch-package" "$root/mismatch-release" "$root/mismatch-install"
cat > "$root/mismatch-package/ask" <<'EOF'
#!/bin/sh
printf '9.9.9\n'
EOF
chmod 755 "$root/mismatch-package/ask"
cp README.md LICENSE "$root/mismatch-package/"
tar -C "$root/mismatch-package" -czf "$root/mismatch-release/$archive_name" ask README.md LICENSE
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$root/mismatch-release" && sha256sum "$archive_name" > "$archive_name.sha256")
else
  (cd "$root/mismatch-release" && shasum -a 256 "$archive_name" > "$archive_name.sha256")
fi
cp "$root/package/ask" "$root/mismatch-install/ask"

if PATH="$root/bin:$PATH" \
  ASK_TEST_RELEASE="$root/mismatch-release" \
  ASK_TEST_VERSION="$version" \
  ASK_INSTALL_DIR="$root/mismatch-install" \
  sh ./install.sh "$version" \
  > "$root/mismatch-stdout" 2> "$root/mismatch-stderr"; then
  exit 1
fi
test "$("$root/mismatch-install/ask" --version)" = "$version"
grep -F "ask: downloaded ask 9.9.9, expected ask $version" "$root/mismatch-stderr" >/dev/null
