#!/usr/bin/env bash
# Build one language data package (SPEC §17.3): scripts/build-langdata.sh <ja-ipadic|ko-kodic> [version]
# Uses the pinned lindera's own dictionary build (the `embed-*` features compile the dictionary source
# into OUT_DIR), then zips the compiled files with langdata.json and the dictionary's licence.
# Output: dist/langdata/langdata-<id>-<version>.zip; prints its sha256 and size for crates/alst-text/langdata.json.
set -euo pipefail

id="${1:?usage: build-langdata.sh <ja-ipadic|ko-kodic> [version]}"
version="${2:-1}"
cd "$(dirname "$0")/.."

case "$id" in
  ja-ipadic) feature=embed-ja; crate=lindera-ipadic; engine=lindera-ipadic ;;
  ko-kodic)  feature=embed-ko; crate=lindera-ko-dic; engine=lindera-ko-dic ;;
  *) echo "unknown package id: $id" >&2; exit 2 ;;
esac

lindera=$(grep -A1 '^name = "lindera"$' Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)

out_dir=$(cargo build -p alst-text --no-default-features --features "$feature" --message-format=json 2>/dev/null \
  | node -e '
    const crate = process.argv[1]; let found = "";
    require("readline").createInterface({ input: process.stdin }).on("line", l => {
      try { const m = JSON.parse(l); if (m.reason === "build-script-executed" && m.package_id.includes("/" + crate + "#") || (m.reason === "build-script-executed" && m.package_id.includes(crate + "@"))) found = m.out_dir; } catch {}
    }).on("close", () => console.log(found));' "$crate")
src="$out_dir/$crate"
[ -f "$src/dict.trie" ] || { echo "compiled dictionary not found under $out_dir" >&2; exit 1; }

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
files=()
for f in "$src"/*; do
  [ -f "$f" ] || continue
  cp "$f" "$stage/"; files+=("$(basename "$f")")
done
crate_src=$(find "$HOME/.cargo/registry/src" -maxdepth 2 -type d -name "$crate-$lindera" | head -1)
cp "$crate_src/NOTICE.txt" "$stage/LICENSE.txt"

list=$(printf '"%s",' "${files[@]}"); list="[${list%,}]"
printf '{"id":"%s","engine":"%s","version":"%s","lindera":"%s","files":%s}\n' "$id" "$engine" "$version" "$lindera" "$list" > "$stage/langdata.json"

mkdir -p dist/langdata
zip_path="$PWD/dist/langdata/langdata-$id-$version.zip"
rm -f "$zip_path"
# Dictionary files are binary and not very compressible; deflate still halves the word table.
(cd "$stage" && zip -q -X -9 "$zip_path" langdata.json LICENSE.txt "${files[@]}")

sha=$(shasum -a 256 "$zip_path" | cut -d' ' -f1)
size=$(wc -c < "$zip_path" | tr -d ' ')
echo "$zip_path"
echo "{\"id\":\"$id\",\"version\":\"$version\",\"sha256\":\"$sha\",\"size\":$size}"
