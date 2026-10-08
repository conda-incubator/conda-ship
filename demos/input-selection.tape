Source demos/_settings.tape

Output demos/input-selection.gif
Output target/input-selection.txt

Require shasum

Hide
Set TypingSpeed 0ms
Type `PS1="$DEMO_PS1" && set -o pipefail && SHIP_INPUT_DEMO_ROOT="$PWD" && SHIP_INPUT_DEMO_BUILDER="$PWD/target/release/cs" && SHIP_INPUT_DEMO_DIR="$(cd "$(mktemp -d /tmp/conda-ship-inputs.XXXXXX)" && pwd -P)"`
Enter
Wait+Line /^\$ *$/
Type `test -x "$SHIP_INPUT_DEMO_BUILDER" && test -x "$SHIP_INPUT_DEMO_ROOT/target/release/cs-template" && "$SHIP_INPUT_DEMO_BUILDER" build --help | grep -q -- --source-lock`
Enter
Wait+Line /^\$ *$/
Type `cp pixi.lock "$SHIP_INPUT_DEMO_DIR/release.lock" && awk '/^\[tool.conda-ship\]$/ { print; print "runtime-name = \"demo\""; print "delegate-executable = \"python\""; next } /^source-environment =/ { print "source-environment = \"default\""; next } { print }' pixi.toml > "$SHIP_INPUT_DEMO_DIR/pixi.toml"`
Enter
Wait+Line /^\$ *$/
Type `printf '%s\n' '[workspace]' 'name = "other"' > "$SHIP_INPUT_DEMO_DIR/conda.toml" && cd "$SHIP_INPUT_DEMO_DIR" && shasum -a 256 conda.toml pixi.toml release.lock > source-inputs.sha256`
Enter
Wait+Line /^\$ *$/
Type `cs() { "$SHIP_INPUT_DEMO_BUILDER" "$@" 2>&1 | sed "s|$SHIP_INPUT_DEMO_DIR/||g"; }`
Enter
Wait+Line /^\$ *$/
Type "clear"
Enter
Wait+Line /^\$ *$/
Sleep 500ms
Set TypingSpeed 40ms
Show

Type "# Explicit inputs with an unreleased source build"
Enter
Type@70ms "ls conda.toml pixi.toml release.lock"
Enter
Wait+Line /^\$ *$/
Sleep 2s

Type@50ms `sed -n '/^\[tool.conda-ship\]/,$p' pixi.toml`
Enter
Wait+Line /^\$ *$/
Sleep 3s
Screenshot target/input-selection-inputs.png

Hide
Type "clear"
Enter
Wait+Line /^\$ *$/
Sleep 500ms
Show

Type "# Choose the manifest, lockfile and solved environment"
Enter
Type@65ms "cs build --manifest pixi.toml --source-lock release.lock --source-environment ship"
Enter
Wait+Line /^\$ *$/
Sleep 3s

Type "# The source inputs stay unchanged"
Enter
Type@65ms "shasum -a 256 --check source-inputs.sha256"
Enter
Wait+Line /^\$ *$/
Sleep 5s
Screenshot target/input-selection-built.png

Hide
Type `cd "$SHIP_INPUT_DEMO_ROOT" && rm -rf "$SHIP_INPUT_DEMO_DIR"`
Enter
Wait+Line /^\$ *$/
